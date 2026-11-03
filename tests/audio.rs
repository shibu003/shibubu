//! 録音（wav の header / 長さ / 録る前の検査 / stop / 上書きしない書き出し）と試聴（afplay）・波形の peak 列の検査。
//! mic は使わない（無音 fixture と KANGAE_AUDIO_FAKE=1）。afplay と osascript は偽物（KANGAE_AFPLAY_BIN / KANGAE_OSASCRIPT_BIN）。
//! REAPER も起こさない。

#[allow(dead_code)]
#[path = "../src/audio/mod.rs"]
mod audio;

use audio::preview::{peaks, sparkline};
use audio::record::{
    inspect, numbered, parse_df, parse_mic_status, preflight, record, record_silence, space_error, to_mono_44k, wait_end, Ended,
    MicPermission, WavInfo, BITS, CHANNELS, SAMPLE_RATE,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("kangae-audio-test-{}-{name}", std::process::id()))
}

/// test ごとに別の空 dir（同じ名前の file を並行の test が触らない）
fn fresh_dir(name: &str) -> PathBuf {
    let d = tmp(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// i16 mono / stereo の fixture wav
fn write_wav(p: &Path, channels: u16, samples: &[i16]) {
    let spec = hound::WavSpec { channels, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(p, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

/// 実行 bit 付きの偽物（1 つを全 test で共有）
fn fake_bin(name: &str, script: &str) -> PathBuf {
    let p = tmp(name);
    std::fs::write(&p, script).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// 偽の afplay: 引数を NUL 区切りで $FAKE_ARGS に、自分の pid を $FAKE_ARGS.pid に書く。$FAKE_SLEEP があれば exec sleep で鳴り続ける
/// ふりをし、無ければ $FAKE_ERR を stderr に出して $FAKE_CODE で終わる。
fn fake_afplay() -> PathBuf {
    static FAKE: OnceLock<PathBuf> = OnceLock::new();
    FAKE.get_or_init(|| {
        fake_bin(
            "afplay.sh",
            "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$FAKE_ARGS\"\necho $$ > \"$FAKE_ARGS.pid\"\n[ -n \"$FAKE_SLEEP\" ] && exec sleep \"$FAKE_SLEEP\"\nprintf '%s' \"$FAKE_ERR\" >&2\nexit \"${FAKE_CODE:-0}\"\n",
        )
    })
    .clone()
}

/// 偽の osascript（mic の許可）: $FAKE_MIC を stdout に出すだけ
fn fake_osascript() -> PathBuf {
    static FAKE: OnceLock<PathBuf> = OnceLock::new();
    FAKE.get_or_init(|| fake_bin("osascript.sh", "#!/bin/sh\nprintf '%s' \"$FAKE_MIC\"\n")).clone()
}

fn rec() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_kangae-rec"));
    c.env_remove("KANGAE_AUDIO_FAKE").env("KANGAE_AFPLAY_BIN", fake_afplay()).env("KANGAE_OSASCRIPT_BIN", fake_osascript());
    c
}

/// stdout を行ごとに channel へ流す（marker の行を待てるように）
fn lines(child: &mut Child) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = tx.send(l);
        }
    });
    rx
}

/// marker を含む行が出るまで待つ（混んだ機械向けに長め）。出ずに閉じたら panic
fn wait_line(rx: &mpsc::Receiver<String>, marker: &str) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(l) = rx.recv_timeout(Duration::from_secs(120)) {
        let hit = l.contains(marker);
        seen.push(l);
        if hit {
            return seen;
        }
    }
    panic!("「{marker}」の行が出ませんでした: {seen:?}");
}

/// child が終わるまで最大 120 秒待つ（終わらなければ kill して panic）
fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let t = Instant::now();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        if t.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            panic!("120 秒で終わりませんでした");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn no_part_files(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".part"))
}

#[test]
fn silence_fixture_writes_44k_16bit_mono_with_exact_length() {
    let p = tmp("silence.wav");
    let info = record_silence(2.0, &p).unwrap();
    assert_eq!(info, WavInfo { sample_rate: SAMPLE_RATE, channels: CHANNELS, bits: BITS, frames: 88_200, peak: 0.0 });
    assert_eq!(inspect(&p).unwrap(), info, "disk 上の header が返り値と一致する");
    let b = std::fs::read(&p).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    assert_eq!(&b[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes([b[22], b[23]]), 1, "channels");
    assert_eq!(u32::from_le_bytes([b[24], b[25], b[26], b[27]]), 44_100, "sample rate");
    assert_eq!(u16::from_le_bytes([b[34], b[35]]), 16, "bits per sample");
    assert_eq!(b.len(), 44 + 88_200 * 2, "header 44 byte + 16bit mono 2 秒の data");
    std::fs::remove_file(&p).ok();
}

#[test]
fn fractional_seconds_round_to_frames() {
    let p = tmp("half.wav");
    assert_eq!(record_silence(0.5, &p).unwrap().frames, 22_050);
    std::fs::remove_file(&p).ok();
}

#[test]
fn stereo_48k_input_becomes_mono_44k() {
    let n = 48_000;
    let cancel: Vec<f32> = (0..n).flat_map(|_| [0.5, -0.5]).collect();
    let out = to_mono_44k(&cancel, 2, 48_000);
    assert_eq!(out.len(), 44_100, "48k の 1 秒 → 44.1k の 1 秒");
    assert!(out.iter().all(|&s| s == 0), "L と R の平均");
    let same: Vec<f32> = (0..n).flat_map(|_| [0.5, 0.5]).collect();
    let out = to_mono_44k(&same, 2, 48_000);
    assert_eq!(out.len(), 44_100);
    let half = (0.5 * i16::MAX as f32).round() as i16;
    assert!(out.iter().all(|&s| s == half), "一定の信号は補間しても一定");
    assert_eq!(to_mono_44k(&vec![1.0f32; 100], 1, 44_100), vec![i16::MAX; 100], "44.1k mono はそのまま");
    assert_eq!(to_mono_44k(&[2.0, -2.0], 1, 44_100), vec![i16::MAX, -i16::MAX], "範囲外は clamp");
    assert!(to_mono_44k(&[], 2, 48_000).is_empty());
}

#[test]
fn cli_fake_env_records_silence_without_mic() {
    let p = tmp("cli.wav");
    let o = Command::new(env!("CARGO_BIN_EXE_kangae-rec"))
        .env("KANGAE_AUDIO_FAKE", "1")
        .args(["--seconds", "1", "--out"])
        .arg(&p)
        .output()
        .unwrap();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    let info = inspect(&p).unwrap();
    assert_eq!((info.sample_rate, info.channels, info.bits, info.frames), (SAMPLE_RATE, CHANNELS, BITS, 44_100));
    assert!(String::from_utf8_lossy(&o.stdout).contains("44100 Hz / 16 bit / 1 ch"));
    std::fs::remove_file(&p).ok();
}

#[test]
fn cli_insert_refuses_missing_wav_before_touching_reaper() {
    let o = Command::new(env!("CARGO_BIN_EXE_kangae-rec"))
        .arg("--insert")
        .arg(tmp("missing.wav"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("as a wav file"));
}

#[test]
fn cli_rejects_unknown_flag_with_usage() {
    let o = Command::new(env!("CARGO_BIN_EXE_kangae-rec")).arg("--bogus").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("使い方"));
}

#[test]
fn peaks_of_fixture_wav_are_fixed() {
    let d = fresh_dir("peaks");
    let p = d.join("ramp.wav");
    write_wav(&p, 1, &[0, 1000, -2000, 3000, -4000, 5000, 32767, -32768]);
    let n = |s: i16| (s as f32 / 32767.0).clamp(-1.0, 1.0);
    assert_eq!(
        peaks(&p, 4).unwrap(),
        vec![(n(0), n(1000)), (n(-2000), n(3000)), (n(-4000), n(5000)), (-1.0, 1.0)],
        "8 frame を 4 本に: 2 frame ずつの min / max"
    );
    assert_eq!(
        peaks(&p, 3).unwrap(),
        vec![(n(-2000), n(1000)), (n(-4000), n(5000)), (-1.0, 1.0)],
        "割り切れない時は frame*本数/frames の切り捨てで本を決める（境目は frame 0 / 3 / 6）"
    );
    let one: Vec<(f32, f32)> = [0, 1000, -2000, 3000, -4000, 5000, 32767, -32768].iter().map(|&s| (n(s), n(s))).collect();
    assert_eq!(peaks(&p, 100).unwrap(), one, "本数が frame より多ければ frame ごと");
    assert!(peaks(&p, 0).unwrap().is_empty());
    let st = d.join("stereo.wav");
    write_wav(&st, 2, &[100, -100, 200, -300]);
    assert_eq!(peaks(&st, 1).unwrap(), vec![(n(-300), n(200))], "stereo は両 channel をまとめて見る");
    let empty = d.join("empty.wav");
    write_wav(&empty, 1, &[]);
    assert!(peaks(&empty, 8).unwrap().is_empty());
    assert!(peaks(&d.join("missing.wav"), 8).unwrap_err().contains("as a wav file"));
}

#[test]
fn sparkline_is_db_scaled() {
    assert_eq!(sparkline(&[(0.0, 0.0), (-1.0, 1.0), (0.0, 0.001), (-0.1, 0.0)]), "▁█▁▆", "無音 / 0 dB / -60 dB / -20 dB");
}

#[test]
fn preflight_refuses_existing_out_and_keeps_it() {
    let d = fresh_dir("exists");
    let p = d.join("take.wav");
    std::fs::write(&p, "keep").unwrap();
    assert!(preflight(1.0, &p).unwrap_err().contains("already exists"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep", "既に在る file に触らない");
    for bad in [0.0, -1.0, 3600.5, f32::NAN] {
        assert!(preflight(bad, &d.join("x.wav")).unwrap_err().contains("Give seconds"), "{bad}");
    }
    assert!(preflight(1.0, &d.join("new/dir/take.wav")).is_ok(), "まだ無い dir は在る祖先の空きを見る");
}

#[test]
fn disk_space_is_read_from_df_and_checked() {
    let mac = "Filesystem   1024-blocks      Used Available Capacity  Mounted on\n/dev/disk1s2   488245288 455370584   9990120    98%    /System/Volumes/Data\n";
    assert_eq!(parse_df(mac), Some(9_990_120 * 1024));
    let spaced = "Filesystem 1024-blocks Used Available Capacity Mounted on\nmap auto_home 0 0 0 100% /System/Volumes/Data/home\n//me@nas/My Share 1000 400 600 40% /Volumes/My Share\n";
    assert_eq!(parse_df(spaced), Some(0), "file system 名に空白があっても Capacity の前の列");
    assert_eq!(parse_df("df: /nope: No such file or directory\n"), None);
    assert!(space_error(300 << 20, 100 << 20).unwrap().contains("Not enough free disk space"));
    assert!(space_error(1 << 20, 1 << 30).is_none());
    assert!(space_error(1 << 20, (1 << 20) + 1).is_some(), "余白（10 MB）も要る");
    assert!(audio::record::free_bytes(&std::env::temp_dir()).unwrap() > 0, "本物の df を読める");
}

#[test]
fn mic_status_values_follow_av_authorization_status() {
    assert_eq!(parse_mic_status("3\n"), MicPermission::Authorized);
    assert_eq!(parse_mic_status("0"), MicPermission::NotDetermined);
    assert_eq!(parse_mic_status("2"), MicPermission::Denied);
    assert_eq!(parse_mic_status("1"), MicPermission::Restricted);
    assert!(matches!(parse_mic_status(""), MicPermission::Unknown(_)));
    assert!(MicPermission::Denied.refusal().unwrap().contains("Microphone"));
    assert!(MicPermission::Authorized.refusal().is_none() && MicPermission::NotDetermined.refusal().is_none());
}

#[test]
fn wait_end_stops_on_lost_device_stop_or_time() {
    let never = AtomicBool::new(false);
    let t = Instant::now();
    let (e, _) = wait_end(30.0, &never, || (t.elapsed() > Duration::from_millis(100)).then(|| "Device disconnected".to_string()));
    assert_eq!(e, Ended::DeviceLost("Device disconnected".to_string()));
    assert!(t.elapsed() < Duration::from_secs(10));
    let (e, d) = wait_end(0.1, &never, || None);
    assert!(e == Ended::Full && d >= Duration::from_millis(100));
    let (e, _) = wait_end(30.0, &AtomicBool::new(true), || None);
    assert_eq!(e, Ended::Stopped);
}

#[test]
fn stop_keeps_what_was_recorded_and_never_clobbers() {
    std::env::set_var("KANGAE_AUDIO_FAKE", "1"); // in-process の record() は mic に触らない
    let d = fresh_dir("stop");
    let p = d.join("take.wav");
    let stop = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    let (p_, stop_) = (&p, &stop);
    let take = std::thread::scope(|s| {
        s.spawn(move || {
            // 録り始めて（preflight の後）から作る。混んだ機械では record が preflight に届く前に 300 ms 過ぎる事がある
            rx.recv_timeout(Duration::from_secs(120)).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(p_, "other").unwrap(); // 録音中に他が同じ名前を作る
            stop_.store(true, Ordering::SeqCst);
        });
        record(30.0, &p, &stop, move || tx.send(()).unwrap()).unwrap()
    });
    assert_eq!(take.ended, Ended::Stopped);
    assert!(take.info.frames > 0 && take.info.frames < 30 * 44_100, "frames={}", take.info.frames);
    assert_eq!(take.path, numbered(&p, 2), "上書きせず -2 に置く");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "other", "他の file はそのまま");
    assert_eq!(inspect(&take.path).unwrap().frames, take.info.frames);
    assert!(take.notes.iter().any(|n| n.contains("wasn't overwritten")), "{:?}", take.notes);
    assert!(no_part_files(&d), "途中の .part を残さない");
}

#[test]
fn failed_write_leaves_nothing() {
    let d = fresh_dir("readonly");
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
    let e = record_silence(0.1, &d.join("take.wav")).unwrap_err();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(e.contains("Can't write") && e.contains("nothing was left behind"), "{e}");
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
}

#[test]
fn cli_enter_stops_and_keeps_the_take() {
    let d = fresh_dir("enter");
    let p = d.join("hum.wav");
    let mut c = rec().env("KANGAE_AUDIO_FAKE", "1").args(["--seconds", "30", "--out"]).arg(&p).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let rx = lines(&mut c);
    wait_line(&rx, "録音中"); // 音が溜まり始めた時に出る行
    std::thread::sleep(Duration::from_millis(500));
    c.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(wait_exit(&mut c).success());
    let out = wait_line(&rx, "次:").join("\n");
    assert!(out.contains("Enter で止めました") && out.contains("波形: "), "{out}");
    let frames = inspect(&p).unwrap().frames;
    assert!(frames > 0 && frames < 30 * 44_100, "frames={frames}");
}

#[test]
fn cli_ctrl_c_cancels_and_leaves_no_file() {
    let d = fresh_dir("ctrlc");
    let p = d.join("hum.wav");
    let mut c = rec().env("KANGAE_AUDIO_FAKE", "1").args(["--seconds", "30", "--out"]).arg(&p).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let rx = lines(&mut c);
    wait_line(&rx, "録音中");
    assert!(Command::new("kill").args(["-INT", &c.id().to_string()]).status().unwrap().success());
    assert_eq!(wait_exit(&mut c).signal(), Some(2), "SIGINT で終わる");
    assert!(!p.exists() && no_part_files(&d), "取り消しは何も残さない");
}

#[test]
fn cli_existing_out_and_denied_mic_record_nothing() {
    let d = fresh_dir("refuse");
    let p = d.join("hum.wav");
    std::fs::write(&p, "keep").unwrap();
    let o = rec().env("KANGAE_AUDIO_FAKE", "1").args(["--seconds", "1", "--out"]).arg(&p).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("already exists"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep");
    // mic を拒否された（AVAuthorizationStatus = 2）時は録らない。KANGAE_AUDIO_FAKE を外しても mic に触る前に止まる
    let q = d.join("denied.wav");
    let o = rec().env("FAKE_MIC", "2").args(["--seconds", "0.1", "--out"]).arg(&q).output().unwrap();
    assert_eq!(o.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Privacy & Security › Microphone"));
    assert!(!q.exists() && no_part_files(&d));
}

static PLAY_N: AtomicUsize = AtomicUsize::new(0);

/// 偽 afplay で --play を走らせる準備（引数の記録先を返す）
fn play_cmd(wav: &Path) -> (Command, PathBuf) {
    let args = tmp(&format!("afplay-args-{}", PLAY_N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_file(&args);
    let mut c = rec();
    c.arg("--play").arg(wav).env("FAKE_ARGS", &args);
    (c, args)
}

fn fixture_take(name: &str) -> PathBuf {
    let p = fresh_dir(name).join("take.wav");
    write_wav(&p, 1, &[0, 1000, -1000, 0]);
    p
}

#[test]
fn cli_play_hands_the_wav_to_afplay() {
    let p = fixture_take("play-ok");
    let (mut c, args) = play_cmd(&p);
    let o = c.output().unwrap();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("試聴中") && out.contains("再生し終えました"), "{out}");
    assert_eq!(std::fs::read_to_string(&args).unwrap(), format!("{}\0", p.display()), "afplay には path だけを渡す");
}

#[test]
fn cli_play_reports_afplay_failure() {
    let p = fixture_take("play-fail");
    let (mut c, _) = play_cmd(&p);
    let o = c.env("FAKE_CODE", "1").env("FAKE_ERR", "Error: AudioFileOpen failed ('dta?')").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("AudioFileOpen failed"), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn cli_play_enter_stops_afplay() {
    let p = fixture_take("play-stop");
    let (mut c, args) = play_cmd(&p);
    let mut child = c.env("FAKE_SLEEP", "90").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let rx = lines(&mut child);
    wait_line(&rx, "試聴中");
    let pid_file = PathBuf::from(format!("{}.pid", args.display()));
    let t = Instant::now();
    while !pid_file.exists() && t.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    let enter = Instant::now();
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(wait_exit(&mut child).success());
    assert!(enter.elapsed() < Duration::from_secs(30), "Enter から {:?}（afplay の 90 秒を待たずに止める）", enter.elapsed());
    assert!(wait_line(&rx, "止めました").iter().any(|l| l.contains("止めました")));
    let alive = Command::new("kill").args(["-0", &pid]).stderr(Stdio::null()).status().unwrap().success();
    assert!(!alive, "afplay（偽物の sleep）が残っていない");
}

#[test]
fn cli_play_refuses_non_wav_without_calling_afplay() {
    let d = fresh_dir("play-bad");
    let p = d.join("not.wav");
    std::fs::write(&p, "not a wav").unwrap();
    let (mut c, args) = play_cmd(&p);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("as a wav file"));
    assert!(!args.exists(), "afplay を呼んでいない");
}

// ---- native handoff（src/audio/surface.rs）----

use audio::surface::{ax_reads, classify, from_window_list, AxAnswer, AxAttr, AxRead, Reader, State, Window, WindowReader, WindowsRead, REAPER};

fn win(id: i64, on: bool, z: Option<i64>) -> Window {
    Window { id, title: format!("w{id}"), on_screen: on, on_current_space: Some(on), z, handle: None }
}

fn read(list: Vec<Window>) -> WindowsRead {
    WindowsRead::Read { list, by: Reader::Bridge, note: None }
}

#[test]
fn surface_states_follow_design_14_2() {
    let s = |installed, pid, w: WindowsRead| classify(&REAPER, installed, pid, &w);
    let unchecked = |why: &str| WindowsRead::Unchecked { why: why.into(), no_reader: true };
    let (st, why, _) = s(false, None, unchecked("x"));
    assert!(st == State::Unsupported && why.contains("reaper.fm/download"), "{why}");
    assert_eq!(s(true, None, unchecked("x")).0, State::AvailableManual, "入っているが未起動");
    let (st, why, _) = s(true, Some(1), unchecked("bridge: 見つかりません"));
    assert!(
        st == State::AvailableManual && why.contains("Its window hasn't been checked (bridge: 見つかりません)") && why.contains("You can still work with it by hand"),
        "読む口が無くても手動は使える・ready は名乗らない・理由を出す: {why}"
    );
    assert_eq!(s(true, Some(1), WindowsRead::Unchecked { why: "16 秒以内に応答がありませんでした".into(), no_reader: false }).0, State::AvailableManual);
    let (st, why, _) = s(true, Some(1), read(vec![]));
    assert!(st == State::Degraded && why.contains("has no window"), "{why}");
    let (st, why, w) = s(true, Some(1), read(vec![win(1, false, Some(9)), win(2, true, Some(3)), win(3, true, Some(5))]));
    assert_eq!((st, w.map(|w| w.id)), (State::Ready, Some(3)), "今の画面の窓のうち一番手前");
    assert!(why.contains("checked with macOS bridge"), "{why}");
    let (st, why, w) = s(true, Some(1), read(vec![win(1, false, None), win(2, false, Some(4))]));
    assert!(st == State::AvailableManual && why.contains("isn't on the current screen") && w.map(|w| w.id) == Some(2), "{why}");
    let tokens: Vec<&str> = [State::Detecting, State::AvailableManual, State::Ready, State::Degraded, State::Unsupported].iter().map(State::as_str).collect();
    assert_eq!(tokens, ["detecting", "available_manual", "ready", "degraded", "unsupported"]);
}

/// W12W-1c: 今の Space か分からない窓（on_current_space = None）は on screen でも ready にしない（分からない時は分からないと持つ）。
#[test]
fn w12w1c_unknown_space_is_not_ready() {
    let unknown = Window { id: 5226, title: "Shibubu".into(), on_screen: true, on_current_space: None, z: Some(1), handle: None };
    assert!(!unknown.here());
    let (st, why, w) = classify(&REAPER, true, Some(1), &read(vec![unknown]));
    assert!(st == State::AvailableManual && why.contains("isn't on the current screen") && w.map(|w| w.id) == Some(5226), "{why}");
    assert!(win(7, true, Some(1)).here() && !win(7, false, Some(1)).here());
}

/// W12W-1c: bridge の window_list の ax_reads[] は、空（答えて 0）・答えない（AXError）・許可無しを別の値で読む（owner 13 通目）。
#[test]
fn w12w1c_ax_reads_separate_empty_from_error() {
    let v = serde_json::json!({ "pid": 4242, "windows": [], "cg_windows": [], "ax_reads": [
        { "attr": "windows", "code": "success", "count": 0, "elapsed_ms": 12 },
        { "attr": "main_window", "code": "ax_cannot_complete", "count": 0, "elapsed_ms": 3200 },
        { "attr": "focused_window", "code": "ax_no_value", "count": 0, "elapsed_ms": 4 },
        { "attr": "focused_window", "code": "not_trusted", "count": 0, "elapsed_ms": 1 },
        { "attr": "windows", "code": "success", "count": 2, "elapsed_ms": 30 },
        { "attr": "sheets", "code": "success", "count": 1, "elapsed_ms": 1 }
    ]});
    let got = ax_reads(&v);
    let want = [
        AxRead { attr: AxAttr::Windows, answer: AxAnswer::Empty, elapsed_ms: 12 },
        AxRead { attr: AxAttr::MainWindow, answer: AxAnswer::Failed("ax_cannot_complete".into()), elapsed_ms: 3200 },
        AxRead { attr: AxAttr::FocusedWindow, answer: AxAnswer::Empty, elapsed_ms: 4 },
        AxRead { attr: AxAttr::FocusedWindow, answer: AxAnswer::NotTrusted, elapsed_ms: 1 },
        AxRead { attr: AxAttr::Windows, answer: AxAnswer::Answered(2), elapsed_ms: 30 },
    ];
    assert_eq!(got, want, "知らない attr（sheets）は捨てる");
    assert_eq!(ax_reads(&serde_json::json!({ "pid": 1, "windows": [] })), Vec::<AxRead>::new(), "前の bridge（ax_reads 無し）は空");
}

/// W12W-1c: 3 属性の bridge の window_list。CG と対応の付かない AX の窓（MainWindow が別の Space の窓を返した）は Space が分からない＝ready にしない・
/// CG の off screen の窓も分からない・答えない属性は note に名前と ms を出す・全部答えた時は note 無し。
#[test]
fn w12w1c_window_list_keeps_unknown_space_and_says_which_attribute_failed() {
    let v = serde_json::json!({ "pid": 4242,
        "windows": [
            { "handle": "w1", "title": "here", "minimized": false, "cg_id": 876 },
            { "handle": "w2", "title": "other space", "minimized": false }
        ],
        "cg_windows": [
            { "id": 876, "pid": 4242, "layer": 0, "order": 3, "on_screen": true, "frame": { "x": 0, "y": 25, "w": 800, "h": 600 } },
            { "id": 893, "pid": 4242, "layer": 0, "order": 40, "on_screen": false, "frame": { "x": 0, "y": 0, "w": 400, "h": 300 } }
        ],
        "ax_reads": [
            { "attr": "windows", "code": "success", "count": 1, "elapsed_ms": 20 },
            { "attr": "main_window", "code": "success", "count": 1, "elapsed_ms": 5 },
            { "attr": "focused_window", "code": "ax_cannot_complete", "count": 0, "elapsed_ms": 6000.4 }
        ]});
    let (list, note) = from_window_list(&v);
    let space: Vec<_> = list.iter().map(|w| (w.id, w.on_screen, w.on_current_space, w.here())).collect();
    assert_eq!(space, [(876, true, Some(true), true), (0, true, None, false), (893, false, None, false)]);
    let note = note.expect("答えない属性は note に出す");
    assert!(note.contains("focused_window isn't answering (ax_cannot_complete · 6000ms)") && !note.contains("main_window"), "{note}");
    // 別の Space の窓しか無い → ready にしない（前は AX の窓を今の画面とみなした）
    let (only_other, _) = from_window_list(&serde_json::json!({ "pid": 4242, "windows": [v["windows"][1].clone()], "cg_windows": [] }));
    assert_ne!(classify(&REAPER, true, Some(4242), &read(only_other)).0, State::Ready);
    // 全部答えた → note 無し・3 つとも空でも CG に窓が無ければ note 無し（窓の無い app）
    let mut all = v.clone();
    all["ax_reads"][2] = serde_json::json!({ "attr": "focused_window", "code": "success", "count": 1, "elapsed_ms": 3 });
    assert_eq!(from_window_list(&all).1, None);
    let empty = serde_json::json!({ "pid": 1, "windows": [], "cg_windows": [], "ax_reads": [
        { "attr": "windows", "code": "success", "count": 0, "elapsed_ms": 1 },
        { "attr": "main_window", "code": "ax_no_value", "count": 0, "elapsed_ms": 1 },
        { "attr": "focused_window", "code": "ax_no_value", "count": 0, "elapsed_ms": 1 }
    ]});
    assert_eq!(from_window_list(&empty).1, None);
    // 3 つとも空なのに CG に窓が在る → note（AX だけ空）
    let mut empty_cg = empty.clone();
    empty_cg["cg_windows"] = v["cg_windows"].clone();
    assert!(from_window_list(&empty_cg).1.is_some_and(|n| n.contains("windows is empty")));
}

/// 2026-10-22 03:44 に W12W-1c の bridge が背景の Shibubu（load 95・App Nap の PRI 4）に返した window_list（帯の CG の窓 5 本は 1 本に略・pid は架空）:
/// kAXWindows と kAXMainWindow は 3 秒で cannot_complete・kAXFocusedWindow だけが窓を返した（前の bridge なら ax_error で AX の窓 0）。
const WINDOW_LIST_W12W1C: &str = r#"{"ax_reads":[{"attr":"windows","code":"ax_cannot_complete","count":0,"elapsed_ms":3132},
 {"attr":"main_window","code":"ax_cannot_complete","count":0,"elapsed_ms":3003},{"attr":"focused_window","code":"success","count":1,"elapsed_ms":1930}],
 "cg_windows":[{"frame":{"h":24,"w":1680,"x":0,"y":0},"id":774,"layer":0,"on_screen":false,"order":1,"owner":"Shibubu","pid":4242,"title":""},
 {"frame":{"h":928,"w":1440,"x":120,"y":32},"id":769,"layer":0,"on_screen":true,"order":6,"owner":"Shibubu","pid":4242,"title":"Shibubu"}],
 "pid":4242,"windows":[{"focused":false,"fullscreen":false,"handle":"w1","main":false,"minimized":false,"pid":4242,"position_settable":false,"role":"",
 "size_settable":false,"subrole":"","title":"Shibubu"}]}"#;

#[test]
fn w12w1c_window_list_of_the_real_bridge_keeps_the_focused_window_when_windows_times_out() {
    let v: serde_json::Value = serde_json::from_str(WINDOW_LIST_W12W1C).unwrap();
    let (list, note) = from_window_list(&v);
    // AX の窓（frame が読めず CG と対応が付かない → Space は分からない）と、CG の on screen の窓（今の画面）
    let got: Vec<_> = list.iter().map(|w| (w.id, w.on_current_space, w.here(), w.handle.as_deref())).collect();
    assert_eq!(got, [(0, None, false, Some("w1")), (769, Some(true), true, None)]);
    let note = note.unwrap();
    assert!(note.contains("windows isn't answering (ax_cannot_complete · 3132ms)") && note.contains("main_window isn't answering") && !note.contains("focused_window"), "{note}");
    assert_eq!(classify(&REAPER, true, Some(4242), &read(list)).0, State::Ready);
}

/// 2026-10-18 に本物の bridge が返した REAPER 7.81 の window_list（AX の窓 1 つ・layer 0 に 1680×24 の帯 4 本・pid は架空）
const WINDOW_LIST_REAPER: &str = r#"{"pid":4242,"windows":[{"handle":"w1","pid":4242,"title":"REAPER v7.81 - EVALUATION LICENSE","role":"AXWindow",
 "subrole":"AXStandardWindow","minimized":false,"main":true,"focused":false,"fullscreen":false,"position_settable":true,"size_settable":true,
 "frame":{"x":840,"y":25,"w":840,"h":952},"cg_id":876,"cg_match":"frame","on_screen":true}],
 "cg_windows":[{"id":892,"pid":4242,"layer":0,"order":1,"on_screen":false,"frame":{"x":0,"y":0,"w":1680,"h":24}},
 {"id":876,"pid":4242,"layer":0,"order":3,"on_screen":true,"frame":{"x":840,"y":25,"w":840,"h":952},"title":"REAPER v7.81 - EVALUATION LICENSE"},
 {"id":891,"pid":4242,"layer":0,"order":20,"on_screen":false,"frame":{"x":0,"y":0,"w":1680,"h":24}},
 {"id":893,"pid":4242,"layer":0,"order":40,"on_screen":false,"frame":{"x":200,"y":200,"w":400,"h":300},"title":"Mixer"}]}"#;

#[test]
fn window_list_of_the_real_bridge_is_read() {
    let v: serde_json::Value = serde_json::from_str(WINDOW_LIST_REAPER).unwrap();
    let (list, note) = from_window_list(&v);
    assert_eq!(note, None);
    // AX の窓 1 つ + AX に無い CG の窓（別の Space の Mixer）。1680×24 の帯は窓に数えない
    assert_eq!(list.iter().map(|w| (w.id, w.on_screen, w.z)).collect::<Vec<_>>(), [(876, true, Some(-3)), (893, false, Some(-40))]);
    assert_eq!(list[0].handle.as_deref(), Some("w1"));
    assert_eq!(classify(&REAPER, true, Some(4242), &read(list)).0, State::Ready);
    // AX が読めない（未許可・app が答えない）→ CG だけで答え、理由を note に
    let mut v2 = v.clone();
    v2["windows"] = serde_json::json!([]);
    v2["ax_error"] = serde_json::json!({ "code": "ax_cannot_complete", "message": "窓の一覧を読めません: app が応答しません" });
    let (list, note) = from_window_list(&v2);
    assert_eq!(list.iter().map(|w| w.id).collect::<Vec<_>>(), [876, 893]);
    assert!(list.iter().all(|w| w.handle.is_none()));
    assert!(note.unwrap().contains("comes only from the window list"));
}

/// 偽の bridge（tests/fake-surface-bridge.py）を env つきで起こす sh
fn fake_bridge(name: &str, env: &[(&str, &str)]) -> PathBuf {
    let py = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake-surface-bridge.py");
    let exports: String = env.iter().map(|(k, v)| format!("export {k}='{v}'\n")).collect();
    fake_bin(name, &format!("#!/bin/sh\n{exports}exec python3 '{}'\n", py.display()))
}

/// 試験の側の罠（YC W18-1）: 呼ばれたら log に 1 行足すだけの偽の cua-driver。製品が `CUA_DRIVER_BIN` を見ない事を log が無い事で確かめる
fn logging_cua(name: &str, log: &Path) -> PathBuf {
    let _ = std::fs::remove_file(log);
    fake_bin(name, &format!("#!/bin/sh\nprintf 'cua %s\\n' \"$*\" >> '{}'\n", log.display()))
}

/// YC W18-1（YC_GATE #14）: 窓を読む口は bridge だけ。bridge が無い・起きない時は未確認（"bridge: " で始まる理由・no_reader = 他に読む口が無い）で、
/// 黙って「窓が無い」にしない。AX の許可が無い bridge は CG だけで答える
#[test]
fn window_reader_is_bridge_only_and_unchecked_without_it() {
    let bridge = fake_bridge("bridge-reader.sh", &[]);
    match WindowReader::with_bridge(Some(bridge)).windows(4242) {
        WindowsRead::Read { list, by, note } => {
            assert_eq!((by, note), (Reader::Bridge, None));
            assert_eq!(list[0].title, "test.rpp - REAPER");
        }
        other => panic!("{other:?}"),
    }
    let unchecked = |r: WindowsRead| match r {
        WindowsRead::Unchecked { why, no_reader } => {
            assert!(no_reader, "bridge の他に読む口は無い: {why}");
            assert!(why.starts_with("bridge: ") && !why.to_lowercase().contains("cua"), "{why}");
        }
        other => panic!("{other:?}"),
    };
    // bridge を使わない
    unchecked(WindowReader::with_bridge(None).windows(4242));
    // bridge が起きない（壊れた実行 file）
    unchecked(WindowReader::with_bridge(Some(fake_bin("bridge-broken.sh", "#!/bin/sh\necho not-json\n"))).windows(4242));
    // AX の許可が無い bridge → CG だけで答える（note に理由）
    let untrusted = fake_bridge("bridge-untrusted.sh", &[("FAKE_MODE", "untrusted")]);
    match WindowReader::with_bridge(Some(untrusted)).windows(4242) {
        WindowsRead::Read { list, by: Reader::Bridge, note: Some(note) } => {
            assert!(note.contains("アクセシビリティの許可がありません"), "{note}");
            assert_eq!(list.len(), 1, "CG の窓（帯は数えない）: {list:?}");
            assert!(list[0].handle.is_none());
        }
        other => panic!("{other:?}"),
    }
}

/// W12W-1c の反証: 本物の bridge の window_list は 3 属性を別々に読み、合計 3 × ax_timeout まで掛かる（run-1 は 3132 + 3003 + 1930ms）。
/// 読み手が「AX の読み 1 回 + 2 秒」しか待たないと、遅い app（この Wave が直す場面）で FocusedWindow の窓が届かず cua-driver か未確認に落ちる
#[test]
fn w12w1c_window_reader_waits_for_the_three_reads_of_window_list() {
    // ax_timeout 2 秒の bridge が 5.8 秒で答える: 1 回分の上限 max(5, 2 + 2) = 5 秒は過ぎ、3 × 2 + 2 = 8 秒には入る
    let slow = fake_bridge("bridge-wl-slow.sh", &[("FAKE_MODE", "slow"), ("FAKE_SLOW", "5.8"), ("FAKE_AX_TIMEOUT", "2")]);
    match WindowReader::with_bridge(Some(slow)).windows(4242) {
        WindowsRead::Read { by: Reader::Bridge, list, .. } => assert_eq!(list[0].title, "test.rpp - REAPER"),
        other => panic!("{other:?}"),
    }
    // 上限は残る: 8 秒を過ぎても答えない bridge は未確認（理由つき・待ち続けない）
    let stuck = fake_bridge("bridge-wl-stuck.sh", &[("FAKE_MODE", "slow"), ("FAKE_SLOW", "10.5"), ("FAKE_AX_TIMEOUT", "2")]);
    match WindowReader::with_bridge(Some(stuck)).windows(4242) {
        WindowsRead::Unchecked { why, .. } => assert!(why.contains("bridge"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn value_reads_go_through_the_bridge_and_say_why_when_they_cannot() {
    let bridge = fake_bridge("bridge-value.sh", &[("FAKE_AX_VALUE", "youtube.com/watch?v=dQw4w9WgXcQ")]);
    let mut r = WindowReader::with_bridge(Some(bridge));
    let WindowsRead::Read { list, .. } = r.windows(4242) else { panic!() };
    assert_eq!(r.value_contains(4242, &list[0], "watch?v=dQw4w9WgXcQ"), Ok(true));
    assert_eq!(r.value_contains(4242, &list[0], "watch?v=other"), Ok(false));
    // AX の handle が無い窓（CG だけの窓）→ 未確認の理由（他の口には落ちない）
    let e = r.value_contains(4242, &win(5, true, None), "watch?v=dQw4w9WgXcQ").unwrap_err();
    assert!(e.starts_with("bridge: ") && e.contains("handle"), "{e}");
    // bridge が無い → 未確認の理由
    let e = WindowReader::with_bridge(None).value_contains(4242, &win(5, true, None), "x").unwrap_err();
    assert!(e.starts_with("bridge: "), "{e}");
}

#[test]
fn cli_surface_prints_a_state_and_exits_by_it() {
    // この Mac に REAPER が在るかで状態は変わる。決まった形で出て、exit code が状態と合う事だけを見る（起動も前に出す事もしない）
    // 罠: CUA_DRIVER_BIN を置いても bridge の他の口は起こさない（YC W18-1）
    let log = tmp("cua-cli-surface.log");
    let fake = logging_cua("cua-driver.sh", &log);
    let o = Command::new(env!("CARGO_BIN_EXE_kangae-surface")).arg("reaper").env("CUA_DRIVER_BIN", &fake).env("KANGAE_SURFACE_BRIDGE", "").output().unwrap();
    assert!(!log.exists(), "cua-driver を呼んだ: {}", std::fs::read_to_string(&log).unwrap_or_default());
    let out = String::from_utf8_lossy(&o.stdout);
    let mut l = out.lines();
    assert!(l.next().unwrap_or("").starts_with("detecting: REAPER を探しています"), "{out}");
    let state = l.next().unwrap_or("").split(':').next().unwrap_or("").to_string();
    assert!(["available_manual", "ready", "degraded", "unsupported"].contains(&state.as_str()), "{out}");
    assert_eq!(o.status.code(), Some(if ["ready", "available_manual"].contains(&state.as_str()) { 0 } else { 1 }), "{out}");
    let o = Command::new(env!("CARGO_BIN_EXE_kangae-surface")).arg("blender").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("reaper / music / youtube"));
}

/// YC W17-1・YC_GATE #17: `kangae-surface` の CLI は偽の bridge と偽の cua-driver が両方在る時も bridge で窓を読み、cua-driver を呼ばない。
/// `probe` は起動中の pid（`pgrep -x REAPER`）が無いと窓を読まないので、REAPER という名前の待つだけの process を起こす（本物の REAPER は起こさない）。
#[test]
fn kangae_surface_reads_the_bridge_even_when_cua_driver_is_there() {
    struct Kill(Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = fresh_dir("surface17");
    // /bin/sleep を写すと platform binary の署名で kill されるので、待つだけの C を REAPER の名前で build する（cc = Xcode の CLT）
    let reaper = dir.join("REAPER");
    let src = dir.join("wait.c");
    std::fs::write(&src, "#include <unistd.h>\nint main(void){sleep(60);return 0;}\n").unwrap();
    assert!(Command::new("cc").arg(&src).arg("-o").arg(&reaper).status().unwrap().success(), "cc で REAPER の名前の process を作れません");
    let _running = Kill(Command::new(&reaper).arg("60").spawn().unwrap());
    let log = tmp("cua-surface17.log");
    let cua = logging_cua("cua-surface17.sh", &log);
    let bridge = fake_bridge("bridge-surface17.sh", &[]);
    let o = Command::new(env!("CARGO_BIN_EXE_kangae-surface")).arg("reaper").env("KANGAE_SURFACE_BRIDGE", &bridge).env("CUA_DRIVER_BIN", &cua).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("窓の確認: macOS bridge"), "bridge で読む: {out}");
    assert!(out.contains("“test.rpp - REAPER”"), "偽の bridge の窓: {out}");
    assert!(!log.exists(), "bridge で足りたのに cua-driver を呼んだ: {}", std::fs::read_to_string(&log).unwrap_or_default());
    assert_eq!(o.status.code(), Some(0), "{out}");
}

#[test]
fn stop_before_any_frame_leaves_nothing() {
    std::env::set_var("KANGAE_AUDIO_FAKE", "1");
    let d = fresh_dir("stop0");
    let p = d.join("take.wav");
    let e = record(30.0, &p, &AtomicBool::new(true), || {}).unwrap_err();
    assert!(e.contains("nothing was left behind"), "{e}");
    assert!(!p.exists() && no_part_files(&d), "0 frame の空の wav を残さない");
}

// ---- REAPER への insert / undo（src/audio/reaper.rs）。REAPER は起こさない: 送る口は偽の REAPER、REAPER の中の script は偽の reaper table で回す ----

use audio::reaper::{lua_safe, parse_insert, parse_undo, script, send, Item, Receipt, INSERT, UNDO, UNDO_PREFIX};

#[test]
fn lua_paths_that_could_run_code_are_refused() {
    for bad in ["/tmp/a]==]os.execute('x')--.wav", "/tmp/end]==", "/tmp/a\nb.wav", "/tmp/a\rb.wav", "/tmp/a\tb.wav"] {
        assert!(lua_safe(Path::new(bad)).unwrap_err().contains("can't go into a REAPER script"), "{bad:?}");
    }
    for ok in ["/tmp/kangae-hum-20261017-130000.wav", "/Users/you/Music/鼻歌 [take 2]=.wav", "/tmp/a]=b]].wav", "/tmp/=]x.wav"] {
        assert_eq!(lua_safe(Path::new(ok)).unwrap(), ok);
    }
    let not_utf8 = Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"/tmp/\xff.wav"));
    assert!(lua_safe(not_utf8).unwrap_err().contains("UTF-8"));
    let s = script("/tmp/x.wav", "/tmp/d", "/tmp/c", "-- body");
    assert!(s.starts_with("local wav = [==[/tmp/x.wav]==]\nlocal done = [==[/tmp/d]==]\nlocal cancel = [==[/tmp/c]==]\n"), "{s}");
    assert_eq!(s.matches("/tmp/x.wav").count(), 1, "path は長文字列の中に 1 回だけ");
}

#[test]
fn cli_insert_and_undo_refuse_lua_breaking_path_before_reaper() {
    let d = fresh_dir("lua-path");
    let called = d.join("reaper-called");
    let fake = fake_bin("reaper-never.sh", &format!("#!/bin/sh\ntouch '{}'\n", called.display()));
    for name in ["evil]==]os.exit(1)--.wav", "line\nbreak.wav"] {
        let p = d.join(name);
        write_wav(&p, 1, &[0, 1, 2, 3]);
        for flag in ["--insert", "--undo"] {
            let o = rec().env("KANGAE_REAPER_BIN", &fake).arg(flag).arg(&p).output().unwrap();
            let err = String::from_utf8_lossy(&o.stderr);
            assert_eq!(o.status.code(), Some(1), "{err}");
            assert!(err.contains("can't go into a REAPER script") && err.contains("nothing was done"), "{flag} {name:?}: {err}");
        }
    }
    assert!(!called.exists(), "REAPER を呼んでいない");
}

#[test]
fn insert_done_lines_become_receipts() {
    let wav = "/tmp/kangae-hum-1.wav";
    let ok = format!("ok\t0\t{{G-1}}\t4.709\t2.5\t120.0\t{wav}\tVocal\tA");
    let item = Item { guid: "{G-1}".into(), track: "Vocal\tA".into(), position: 4.709, length: 2.5, source: wav.into(), new_track: false, project_bpm: 120.0 };
    assert_eq!(parse_insert(&ok, wav, 2.5).unwrap(), Receipt::Verified(item.clone()), "track 名の tab はそのまま");
    let r = parse_insert(&ok.replacen("\t0\t", "\t1\t", 1), wav, 2.495).unwrap();
    assert!(matches!(&r, Receipt::Verified(i) if i.new_track), "mode 1 = 新しい track・長さは 10 ms まで許す: {r:?}");
    let r = parse_insert(&ok, wav, 3.0).unwrap();
    assert!(matches!(&r, Receipt::Unverified { reason, .. } if reason.contains("s long")), "{r:?}");
    let r = parse_insert(&ok, "/tmp/other.wav", 2.5).unwrap();
    assert!(matches!(&r, Receipt::Unverified { reason, .. } if reason.contains("source")), "{r:?}");
    assert_eq!(parse_insert(&ok.replacen("ok", "dup", 1), wav, 99.0).unwrap(), Receipt::Duplicate(item), "dup は既に在る item をそのまま返す");
    assert!(parse_insert("err\titem が増えませんでした", wav, 1.0).unwrap_err().contains("item が増えませんでした"));
    assert!(parse_insert("ok\t0\t{G}", wav, 1.0).unwrap_err().contains("Couldn't read REAPER's reply"));
    assert!(parse_insert("", wav, 1.0).unwrap_err().contains("Couldn't read REAPER's reply"));
    assert_eq!(Receipt::EffectUnknown { reason: String::new() }.kind(), "effect_unknown");
}

#[test]
fn undo_done_lines_say_what_happened() {
    let desc = format!("{UNDO_PREFIX}kangae-hum-1.wav {{G-1}}");
    assert_eq!(parse_undo(&format!("ok\t{desc}\t{desc}")).unwrap(), desc);
    assert!(parse_undo(&format!("unverified\t{desc}")).unwrap_err().contains("is still there"));
    let e = parse_undo("refused\tother\tMove items").unwrap_err();
    assert!(e.contains("“Move items”") && e.contains("nothing was done"), "{e}");
    assert!(parse_undo("refused\tother\t").unwrap_err().contains("There's nothing that can be undone"));
    assert!(parse_undo(&format!("refused\tanother\t{desc}")).unwrap_err().contains("a different take"));
    assert!(parse_undo("err\tboom").unwrap_err().contains("boom"));
}

/// 偽の REAPER: `-nonewinst <script>` の script の `local done = [==[…]==]` へ reply を書く（本物の script と同じく tmp に書いて mv。
/// 直に書くと、shell が空の file を作った瞬間を send が読んで "" を返す）
fn fake_reaper(name: &str, reply: &str) -> PathBuf {
    fake_bin(name, &format!("#!/bin/sh\ndone=$(sed -n 's/^local done = \\[==\\[\\(.*\\)\\]==\\]$/\\1/p' \"$2\")\nprintf '%s\\n' '{reply}' > \"$done.tmp\" && mv \"$done.tmp\" \"$done\"\n"))
}

/// この process の send が temp dir に残した dir
fn send_dirs() -> Vec<PathBuf> {
    let mine = format!("kangae-rec-{}-", std::process::id());
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(&mine))
        .map(|e| e.path())
        .collect()
}

#[test]
fn send_reads_the_done_file_and_leaves_cancel_on_timeout() {
    let reply = "ok\t0\t{G}\t0\t1\t120\t/x.wav\tT";
    let ok = fake_reaper("reaper-ok.sh", reply);
    let r = send(&ok, |done, cancel| script("/x.wav", done, cancel, "-- body"), Duration::from_secs(60)).unwrap();
    assert_eq!(r.as_deref(), Some(reply));
    assert!(send_dirs().is_empty(), "応答を読んだら temp dir を消す: {:?}", send_dirs());
    // 応答しない REAPER（混んだ機械では偽物が動き出す前に 1 秒が過ぎる事もあるが、どちらでも done は来ない）
    let silent = fake_bin("reaper-silent.sh", "#!/bin/sh\nexit 0\n");
    let t = Instant::now();
    let r = send(&silent, |done, cancel| script("/x.wav", done, cancel, "-- body"), Duration::from_secs(1)).unwrap();
    assert_eq!(r, None, "応答が無ければ None（insert はこれを EffectUnknown にする）");
    assert!(t.elapsed() < Duration::from_secs(30));
    let left = send_dirs();
    assert!(left.len() == 1 && left[0].join("cancel").exists() && left[0].join("kangae.lua").exists(), "遅れて走った script が何もしないよう cancel を置いて temp dir を残す: {left:?}");
    std::fs::remove_dir_all(&left[0]).ok();
}

/// REAPER の API のうち script が使う物の偽物（item は 1 take・選択 track 在り・undo は block ごとに item 列を控える）
const MOCK_REAPER: &str = r#"
local items, undo, redo, n = {}, {}, nil, 0
local function copy(t) local c = {} for i, v in ipairs(t) do c[i] = v end return c end
reaper = {}
function reaper.CountMediaItems() return #items end
function reaper.GetMediaItem(_, i) return items[i + 1] end
function reaper.GetSetMediaItemInfo_String(it) return true, it.guid end
function reaper.CountTakes() return 1 end
function reaper.GetTake(it) return it end
function reaper.GetActiveTake(it) return it end
function reaper.GetMediaItemTake_Source(tk) return tk.src end
function reaper.GetMediaSourceFileName(s) return s end
function reaper.GetMediaItemInfo_Value(it, k) if k == "D_POSITION" then return it.pos end return it.len end
function reaper.GetMediaItem_Track() return nil end
function reaper.TimeMap2_GetDividedBpmAtTime() return 120 end
function reaper.CountSelectedTracks() return 1 end
function reaper.InsertMedia(f) n = n + 1; table.insert(items, {guid = "{G" .. n .. "}", src = f, pos = 0, len = 1}) end
local snap
function reaper.Undo_BeginBlock2() snap = copy(items) end
function reaper.Undo_EndBlock2(_, d) table.insert(undo, {d, snap}) end
function reaper.Undo_CanUndo2() return undo[#undo] and undo[#undo][1] end
function reaper.Undo_DoUndo2() local u = table.remove(undo) if not u then return 0 end items = u[2]; redo = u[1]; return 1 end
function reaper.Undo_CanRedo2() return redo end
function reaper.UpdateArrange() end
local function step(f, done) os.remove(done); dofile(f); local h = assert(io.open(done)); io.write(h:read("*a"), "\n"); h:close() end
"#;

#[test]
fn reaper_side_lua_dups_reads_back_and_undoes_only_its_own_insert() {
    let Some(lua) = ["luajit", "lua", "lua5.4"].into_iter().find(|b| Command::new(b).arg("-v").output().is_ok()) else {
        eprintln!("skip: Lua の実行 file が無い（luajit / lua）。REAPER の中の script は実物の通しでだけ確かめる");
        return;
    };
    let d = fresh_dir("lua-mock");
    let p = |n: &str| d.join(n).display().to_string();
    let (a, b) = (p("kangae-hum-a.wav"), p("kangae-hum-b.wav"));
    let file = |name: &str, wav: &str, body: &str| {
        std::fs::write(p(name), script(wav, &p(&format!("{name}.done")), &p("cancel"), body)).unwrap();
        format!("step([==[{}]==], [==[{}]==])\n", p(name), p(&format!("{name}.done")))
    };
    let (ins_a, ins_b, undo_a, undo_b) = (file("ins-a.lua", &a, INSERT), file("ins-b.lua", &b, INSERT), file("undo-a.lua", &a, UNDO), file("undo-b.lua", &b, UNDO));
    let cancel = p("cancel");
    let driver = format!(
        "{MOCK_REAPER}{ins_a}{ins_a}table.insert(undo, {{\"Move items\", copy(items)}})\n{undo_a}table.remove(undo)\n{ins_b}{undo_a}{undo_b}{undo_a}\
         io.write(#items, \"\\n\")\nio.open([==[{cancel}]==], \"w\"):close()\n{ins_a}io.write(#items, \"\\n\")\n"
    );
    std::fs::write(p("driver.lua"), driver).unwrap();
    let o = Command::new(lua).arg(p("driver.lua")).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(o.status.success(), "{out}{}", String::from_utf8_lossy(&o.stderr));
    let l: Vec<&str> = out.lines().collect();
    assert_eq!(l.len(), 10, "{out}");
    assert!(matches!(parse_insert(l[0], &a, 1.0).unwrap(), Receipt::Verified(i) if i.guid == "{G1}"), "{}", l[0]);
    assert!(matches!(parse_insert(l[1], &a, 1.0).unwrap(), Receipt::Duplicate(i) if i.guid == "{G1}"), "同じ take の 2 回目は入れない: {}", l[1]);
    assert!(parse_undo(l[2]).unwrap_err().contains("“Move items”"), "人の操作が最後なら undo しない: {}", l[2]);
    assert!(matches!(parse_insert(l[3], &b, 1.0).unwrap(), Receipt::Verified(i) if i.guid == "{G2}"), "{}", l[3]);
    assert!(parse_undo(l[4]).unwrap_err().contains("a different take"), "最後がa different take の insert なら undo しない: {}", l[4]);
    assert_eq!(parse_undo(l[5]).unwrap(), format!("{UNDO_PREFIX}kangae-hum-b.wav {{G2}}"));
    assert_eq!(parse_undo(l[6]).unwrap(), format!("{UNDO_PREFIX}kangae-hum-a.wav {{G1}}"));
    assert_eq!(l[7], "0", "2 つの insert を 1 回ずつ戻して item は 0");
    assert!(parse_insert(l[8], &a, 1.0).unwrap_err().contains("取り消し済み"), "cancel が在れば何もしない: {}", l[8]);
    assert_eq!(l[9], "0");
}

// ---- 録った wav の BPM と「project の tempo に合わせますか」の提案（src/audio/mod.rs の `tempo`）----

use audio::tempo::{estimate, estimate_bpm, propose, Bpm, MIN_SECONDS};

/// bpm の click（1 kHz・30 ms で減衰する burst）を seconds 秒。44.1k の -1.0〜1.0
fn clicks(bpm: f64, seconds: f64) -> Vec<f32> {
    let rate = 44_100.0;
    (0..(seconds * rate) as usize)
        .map(|i| {
            let t = i as f64 / rate;
            let since = t % (60.0 / bpm);
            if since < 0.03 { (0.8 * (-since / 0.01).exp() * (2.0 * std::f64::consts::PI * 1000.0 * since).sin()) as f32 } else { 0.0 }
        })
        .collect()
}

#[test]
fn bpm_of_click_fixtures_is_found() {
    for bpm in [72.0, 100.0, 140.0] {
        let got = estimate(clicks(bpm, 8.0), 44_100).unwrap_or_else(|| panic!("{bpm} BPM が読めない"));
        assert!((got.bpm as f64 - bpm).abs() < 2.0 && got.confidence >= 0.1, "{bpm} → {got:?}");
    }
    // 120 から 1 octave の外は半分 / 倍に寄る（click の 180 と 90 は自己相関では区別できない）。propose が倍 / 半分を折るので提案は同じ
    let got = estimate(clicks(180.0, 8.0), 44_100).unwrap();
    assert!((got.bpm - 90.0).abs() < 2.0, "{got:?}");
    assert_eq!(propose(got, 180.0), None, "倍で数えれば合っている");
    // 2 秒周期で膨らむ伴奏（220 Hz）の上の click: 音量そのものの自己相関は膨らみに引っ張られて短い lag（200 BPM）に寄る。
    // 増え分（onset）で見るので拍が読める
    let tau = 2.0 * std::f32::consts::PI;
    let swell: Vec<f32> = clicks(100.0, 8.0)
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let t = i as f32 / 44_100.0;
            c + 0.6 * (0.5 + 0.5 * (tau * t / 2.0).sin()) * (tau * 220.0 * t).sin()
        })
        .collect();
    let got = estimate(swell, 44_100).unwrap();
    assert!((got.bpm - 100.0).abs() < 2.0, "膨らむ伴奏の上の 100 BPM: {got:?}");
    assert_eq!(estimate(vec![0.0; 44_100 * 8], 44_100), None, "無音は読まない");
    assert_eq!(estimate(clicks(100.0, MIN_SECONDS as f64 - 1.0), 44_100), None, "{MIN_SECONDS} 秒未満は読まない");
    let d = fresh_dir("bpm");
    let p = d.join("clicks.wav");
    let s: Vec<i16> = clicks(100.0, 8.0).iter().map(|&x| (x * 32767.0) as i16).collect();
    write_wav(&p, 1, &s);
    let got = estimate_bpm(&p).unwrap().unwrap();
    assert!((got.bpm - 100.0).abs() < 2.0, "wav から: {got:?}");
    assert!(estimate_bpm(&d.join("missing.wav")).unwrap_err().contains("as a wav file"));
}

#[test]
fn tempo_proposal_folds_and_says_what_changes() {
    let b = |bpm: f32| Bpm { bpm, confidence: 0.5 };
    let p = propose(b(100.0), 120.0).unwrap();
    assert_eq!((p.fold, p.take_bpm), (1.0, 100.0));
    assert!((p.playrate - 1.2).abs() < 1e-9, "{}", p.playrate);
    assert_eq!(p.text, "Match the recording (about 100 BPM) to the project's tempo (120 BPM)?");
    assert!(p.what_changes.contains("1.200 times the speed") && p.what_changes.contains("Undo"), "{}", p.what_changes);
    assert!(p.why.contains("volume (confidence 0.50)"), "{}", p.why);
    let p = propose(b(61.0), 125.0).unwrap();
    assert_eq!(p.fold, 2.0, "半分の速さで歌った鼻歌は倍で数える");
    assert!(p.why.contains("volume (about 61 BPM counted at double tempo, so about 122 BPM) (confidence"), "{}", p.why);
    assert_eq!(propose(b(119.0), 120.0), None, "2% 未満のずれは提案しない");
    assert_eq!(propose(b(240.5), 120.0), None, "倍で合っていれば提案しない");
    assert_eq!(propose(b(100.0), 92.5).unwrap().text, "Match the recording (about 100 BPM) to the project's tempo (92.5 BPM)?");
    assert_eq!(propose(b(100.0), 0.0), None);
    assert_eq!(propose(b(100.0), f64::NAN), None);
}
