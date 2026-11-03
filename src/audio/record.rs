//! mic から録って wav に書く。device の既定（48k / stereo など）で受け、`to_mono_44k` で 44.1k / 16bit / mono に揃える。
//! `KANGAE_AUDIO_FAKE=1` なら mic に触らず無音を録る（実時間で進むので stop も本物と同じ道を通る。test 用）。
//! 失敗の扱い（docs/diagrams.md §10）: 録る前に「保存先が既に在る」「disk の空きが足りない」を見て、何も作らずに止める。
//! mic の許可（TCC）は `mic_permission` で別に聞く（UI は録音欄を開いた時、CLI は録る前）。cpal は許可が無くても error を出さず無音を返す。
//! 録音の終わり方は 3 つ: 秒数に届く / stop（UI の停止・CLI の Enter）= 録れた分を残す / 入力 device が外れた = そこまでを残す。
//! 録れた音は書き出すまで RAM だけに在るので、process ごと止まれば（CLI の Ctrl-C）file は何も残らない。
//! 書き出しは同じ dir の `.part` に書き、読めるのを確かめてから、既存の file を上書きしない形で置く。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 44_100;
pub const BITS: u16 = 16;
pub const CHANNELS: u16 = 1;
/// 秒数の上限（1 時間）。録音は書き出すまで RAM に溜めるので、これより長い物は録らない
pub const MAX_SECONDS: f32 = 3600.0;
/// disk の空きの見積もりに足す余白（file system の管理分）
const DISK_SLACK: u64 = 10 << 20;

/// 書いた（または読んだ）wav の要約。
#[derive(Debug, Clone, PartialEq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    /// frame 数（mono なら sample 数と同じ）
    pub frames: u32,
    /// 最大振幅（0.0〜1.0）。0.0 なら全て無音 = mic の許可が無い・mute の可能性
    pub peak: f32,
}

impl WavInfo {
    pub fn seconds(&self) -> f32 {
        self.frames as f32 / self.sample_rate as f32
    }
}

/// 録音の終わり方。
#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    /// 指定の秒数を録り切った
    Full,
    /// stop で止めた（録れた分を残す）
    Stopped,
    /// 録音中に入力 device が外れた（cpal の原文）。そこまでを残す
    DeviceLost(String),
}

/// 録れた take。
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// 書いた file（保存先が録音中に他から作られたら `-2` などの別名）
    pub path: PathBuf,
    pub info: WavInfo,
    pub ended: Ended,
    /// 録音中の注意（音の途切れの回数・既定の入力 device が替わった・別名で残した）
    pub notes: Vec<String>,
}

/// mic の使用の許可（macOS の TCC。AVFoundation の AVAuthorizationStatus）。
#[derive(Debug, Clone, PartialEq)]
pub enum MicPermission {
    Authorized,
    /// まだ聞かれていない。録り始めると macOS が dialog で聞く（押すまでは無音で録れる）
    NotDetermined,
    /// 本人が断った
    Denied,
    /// 管理者の設定（MDM など）で使えない
    Restricted,
    /// 確かめられなかった（理由）。録音は止めない
    Unknown(String),
}

impl MicPermission {
    /// 録っても無音にしかならない時（Denied / Restricted）の、次の一手つきの文。
    pub fn refusal(&self) -> Option<String> {
        match self {
            MicPermission::Denied => Some("Microphone use isn't allowed. In System Settings › Privacy & Security › Microphone, turn on this terminal (or Shibubu), then record again (nothing was recorded)".to_string()),
            MicPermission::Restricted => Some("The microphone is disabled by an administrator setting on this Mac (nothing was recorded). Ask the administrator to allow Microphone".to_string()),
            _ => None,
        }
    }
}

/// AVCaptureDevice の authorizationStatusForMediaType("soun") を JXA で聞く（crate を足さずに AVFoundation を読む。
/// `ObjC.import('AVFoundation')` では class が出ないので bundle を load して NSClassFromString で引く）。
/// 許可は「責任のある process」（terminal・Kangae.app）ごとで、子の osascript は親と同じ答えを返す。
const MIC_STATUS: &str = r#"$.NSBundle.bundleWithPath("/System/Library/Frameworks/AVFoundation.framework").load; $.NSClassFromString("AVCaptureDevice").authorizationStatusForMediaType($("soun"))"#;

/// osascript の実行 file。`KANGAE_OSASCRIPT_BIN` で差し替え可（test は偽物を置く。src/media と同じ変数）。
pub fn osascript_bin() -> PathBuf {
    std::env::var_os("KANGAE_OSASCRIPT_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/bin/osascript"))
}

/// mic の許可を聞く。osascript が起きない・timeout・読めない答えは Unknown（録音は止めない）。混んだ機械では 4〜7 秒かかる。
pub fn mic_permission(timeout: Duration) -> MicPermission {
    let bin = osascript_bin();
    let mut child = match Command::new(&bin)
        .args(["-l", "JavaScript", "-e", MIC_STATUS])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return MicPermission::Unknown(format!("Couldn't start {}: {e}", bin.display())),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return MicPermission::Unknown(format!("No answer within {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return MicPermission::Unknown(e.to_string()),
        }
    }
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut s, &mut out);
    }
    parse_mic_status(&out)
}

/// MIC_STATUS の stdout: 0 = notDetermined / 1 = restricted / 2 = denied / 3 = authorized（AVAuthorizationStatus の値）。
pub fn parse_mic_status(stdout: &str) -> MicPermission {
    match stdout.trim() {
        "3" => MicPermission::Authorized,
        "0" => MicPermission::NotDetermined,
        "2" => MicPermission::Denied,
        "1" => MicPermission::Restricted,
        other => MicPermission::Unknown(format!("Can't read the answer: {}", other.chars().take(200).collect::<String>())),
    }
}

/// `KANGAE_AUDIO_FAKE` が付いていれば mic を使わない（"" と "0" は付いていない扱い）。
pub fn fake_enabled() -> bool {
    std::env::var("KANGAE_AUDIO_FAKE").map(|v| !v.is_empty() && v != "0").unwrap_or(false)
}

fn frames_for(seconds: f32) -> usize {
    (seconds.max(0.0) * SAMPLE_RATE as f32).round() as usize
}

/// 無音 fixture: seconds 秒の 0 を 44.1k / 16bit / mono で書く（すぐ返る。mic に触らない）。
pub fn record_silence(seconds: f32, out: &Path) -> Result<WavInfo, String> {
    write_mono(out, &vec![0i16; frames_for(seconds)]).map(|(_, info)| info)
}

/// 録る前の検査（何も作らない）: 保存先が既に在れば止める（上書きしない）・disk の空きが足りなければ止める。
pub fn preflight(seconds: f32, out: &Path) -> Result<(), String> {
    if !(seconds > 0.0 && seconds <= MAX_SECONDS) {
        return Err(format!("Give seconds greater than 0 and at most {MAX_SECONDS}: {seconds}"));
    }
    if out.exists() {
        return Err(format!("{} already exists (it won't be overwritten). Give another name with --out", out.display()));
    }
    let needed = wav_bytes(frames_for(seconds) as u64);
    let avail = free_bytes(&existing_dir(out))?;
    match space_error(needed, avail) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// 16bit / mono の wav の大きさ（header 44 byte + 2 byte × frame）。
pub fn wav_bytes(frames: u64) -> u64 {
    44 + frames * 2
}

/// 空き avail で needed を書けないなら、人の言葉の理由。
pub fn space_error(needed: u64, avail: u64) -> Option<String> {
    let mb = |b: u64| b as f64 / 1_048_576.0;
    (avail < needed + DISK_SLACK).then(|| {
        format!(
            "Not enough free disk space for the destination ({:.1} MB needed · {:.1} MB free). Nothing was recorded. Free some space, record fewer seconds, or give another disk with --out",
            mb(needed + DISK_SLACK),
            mb(avail)
        )
    })
}

/// out の dir のうち、今在る一番近い祖先（まだ無い dir は書く時に作る）。
fn existing_dir(out: &Path) -> PathBuf {
    let mut d = out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    while !d.is_dir() {
        match d.parent() {
            Some(p) if !p.as_os_str().is_empty() => d = p.to_path_buf(),
            _ => return PathBuf::from("."),
        }
    }
    d
}

/// dir の在る disk の空き（byte）。`df -kP`（POSIX の出力）を読む。
pub fn free_bytes(dir: &Path) -> Result<u64, String> {
    let o = Command::new("/bin/df")
        .arg("-kP")
        .arg(dir)
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Can't check free disk space (df): {e}"))?;
    parse_df(&String::from_utf8_lossy(&o.stdout)).ok_or_else(|| format!("Can't check free disk space (can't read the output of df: {})", dir.display()))
}

/// `df -kP` の 2 行目の Available（KB）を byte で。file system 名と mount 先に空白が在っても読めるよう、
/// 「NN%」（Capacity）の 1 つ前の列を取る。
pub fn parse_df(stdout: &str) -> Option<u64> {
    let cols: Vec<&str> = stdout.lines().nth(1)?.split_whitespace().collect();
    let cap = cols.iter().position(|c| c.ends_with('%') && c[..c.len() - 1].chars().all(|ch| ch.is_ascii_digit()))?;
    cols.get(cap.checked_sub(1)?)?.parse::<u64>().ok().map(|kb| kb * 1024)
}

/// 録音の終わりを待つ: 秒数に届く / stop が立つ / lost が理由を返す、の早い方。20 ms ごとに見る。戻り値は (終わり方, 経った時間)。
pub fn wait_end(seconds: f32, stop: &AtomicBool, lost: impl Fn() -> Option<String>) -> (Ended, Duration) {
    let started = Instant::now();
    let total = Duration::from_secs_f32(seconds.max(0.0));
    loop {
        let t = started.elapsed();
        if let Some(why) = lost() {
            return (Ended::DeviceLost(why), t);
        }
        if stop.load(Ordering::SeqCst) {
            return (Ended::Stopped, t);
        }
        if t >= total {
            return (Ended::Full, t);
        }
        std::thread::sleep(Duration::from_millis(20).min(total - t));
    }
}

/// 既定の入力 device から最大 seconds 秒録り、44.1k / 16bit / mono の wav を out に書く。stop が立てばそこまでを書く。
/// started は音が実際に溜まり始めた時に 1 回呼ぶ（UI はここで「録音中」に変える。混んだ機械では検査と device を開くのに数秒かかる）。
pub fn record(seconds: f32, out: &Path, stop: &AtomicBool, started: impl FnOnce()) -> Result<Take, String> {
    preflight(seconds, out)?;
    // 検査と device を開く間（混んだ機械では数秒）に止められていれば録り始めない（UI の「準備中に停止」）
    if stop.load(Ordering::SeqCst) {
        return Err("Stopped before recording started (nothing was left behind). Record again".to_string());
    }
    if fake_enabled() {
        started();
        let (ended, t) = wait_end(seconds, stop, || None);
        let frames = if ended == Ended::Full { frames_for(seconds) } else { frames_for(t.as_secs_f32().min(seconds)) };
        return finish(out, &vec![0i16; frames], ended, Vec::new());
    }
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("No input device was found (nothing was recorded). Connect a microphone, or check System Settings › Sound › Input")?;
    let name = device_name(&device);
    let config = device
        .default_input_config()
        .map_err(|e| format!("Can't read the settings of the input device “{name}”: {e}"))?;
    let (in_rate, in_channels, format) = (config.sample_rate(), config.channels(), config.sample_format());
    let stream_config: cpal::StreamConfig = config.into();
    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let errors: Arc<Mutex<Vec<cpal::Error>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&errors);
    let err_fn = move |e: cpal::Error| {
        if let Ok(mut v) = sink.lock() {
            v.push(e);
        }
    };
    let stream = match format {
        cpal::SampleFormat::I8 => build::<i8>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::I16 => build::<i16>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::I32 => build::<i32>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::I64 => build::<i64>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::U8 => build::<u8>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::U16 => build::<u16>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::U32 => build::<u32>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::F32 => build::<f32>(&device, stream_config, &buf, err_fn),
        cpal::SampleFormat::F64 => build::<f64>(&device, stream_config, &buf, err_fn),
        other => return Err(format!("This microphone's audio format isn't supported ({other})")),
    }
    .map_err(|e| format!("Can't create a recording stream for the input device “{name}”: {e}"))?;
    stream.play().map_err(|e| format!("Can't start recording: {e}"))?;
    started();
    // device が外れると cpal（coreaudio）は stream を止めて DeviceNotAvailable を error callback に出す
    let (ended, _) = wait_end(seconds, stop, || {
        errors.lock().ok()?.iter().find(|e| matches!(e.kind(), cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated)).map(|e| e.to_string())
    });
    drop(stream);
    let raw = std::mem::take(&mut *buf.lock().map_err(|_| "Something went wrong inside Shibubu while recording. Record again".to_string())?);
    let mut mono = to_mono_44k(&raw, in_channels, in_rate);
    mono.truncate(frames_for(seconds));
    let mut notes = Vec::new();
    let xruns = errors.lock().map(|v| v.iter().filter(|e| e.kind() == cpal::ErrorKind::Xrun).count()).unwrap_or(0);
    if xruns > 0 {
        notes.push(format!("The sound dropped out {xruns} times while recording (this happens when the Mac is busy; record again if it bothers you)"));
    }
    if let Some(now) = host.default_input_device().map(|d| device_name(&d)) {
        if now != name {
            notes.push(format!("The default input device changed from “{name}” to “{now}” while recording (this take was recorded with “{name}”)"));
        }
    }
    if mono.is_empty() {
        return Err(match ended {
            Ended::DeviceLost(why) => format!("The input device “{name}” was disconnected right after recording started ({why}). Nothing was left behind. Reconnect the microphone and record again"),
            Ended::Stopped => "Stopped before recording started (nothing was left behind). Record again".to_string(),
            Ended::Full => format!("No sound arrived from the input device “{name}” (nothing was left behind). Record again"),
        });
    }
    finish(out, &mono, ended, notes)
}

fn device_name(device: &cpal::Device) -> String {
    use cpal::traits::DeviceTrait;
    device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "(unknown name)".to_string())
}

fn finish(out: &Path, samples: &[i16], ended: Ended, mut notes: Vec<String>) -> Result<Take, String> {
    if samples.is_empty() {
        return Err("Stopped before recording started (nothing was left behind). Record again".to_string());
    }
    let (path, info) = write_mono(out, samples)?;
    if path != out {
        notes.push(format!("{} was created elsewhere while recording, so it wasn't overwritten and the take was saved as {}", out.display(), path.display()));
    }
    Ok(Take { path, info, ended, notes })
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buf: &Arc<Mutex<Vec<f32>>>,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    use cpal::Sample;
    let buf = Arc::clone(buf);
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if let Ok(mut b) = buf.lock() {
                b.extend(data.iter().map(|&s| f32::from_sample(s)));
            }
        },
        err_fn,
        None,
    )
}

/// interleaved な samples（channels ch・rate Hz）を mono 44.1k の i16 に揃える。ch は平均、rate は線形補間。
// ponytail: 線形補間のみ（anti-alias filter 無し）。鼻歌（2 kHz 以下）には足りる。濁りが聞こえたら windowed-sinc（rubato 等）へ
pub fn to_mono_44k(samples: &[f32], channels: u16, rate: u32) -> Vec<i16> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples
        .chunks_exact(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    let resampled: Vec<f32> = if rate == SAMPLE_RATE || mono.is_empty() {
        mono
    } else {
        let n = (mono.len() as f64 * SAMPLE_RATE as f64 / rate as f64).round() as usize;
        let step = rate as f64 / SAMPLE_RATE as f64;
        let last = mono.len() - 1;
        (0..n)
            .map(|i| {
                let x = i as f64 * step;
                let k = x.floor() as usize;
                let t = (x - k as f64) as f32;
                let a = mono[k.min(last)];
                let b = mono[(k + 1).min(last)];
                a + (b - a) * t
            })
            .collect()
    };
    resampled
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)
        .collect()
}

/// wav の sample を -1.0〜1.0 の f32 で順に（整数は 2^(bits-1)-1 で割る。channel は interleaved のまま）。読めない sample で止まる。
pub fn norm_samples<'a, R: std::io::Read>(r: &'a mut hound::WavReader<R>) -> Box<dyn Iterator<Item = f32> + 'a> {
    let spec = r.spec();
    match spec.sample_format {
        hound::SampleFormat::Int => {
            let full = ((1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) - 1).max(1) as f32;
            Box::new(r.samples::<i32>().map_while(Result::ok).map(move |s| (s as f32 / full).clamp(-1.0, 1.0)))
        }
        hound::SampleFormat::Float => Box::new(r.samples::<f32>().map_while(Result::ok)),
    }
}

/// wav を開く（読めなければ人の言葉の error）。
pub fn open(path: &Path) -> Result<hound::WavReader<std::io::BufReader<std::fs::File>>, String> {
    hound::WavReader::open(path).map_err(|e| format!("Can't read {} as a wav file: {e}", path.display()))
}

/// 既存の wav の header と長さ・peak を読む（`--play` / `--insert` の前検査と、書いた直後の確認）。
pub fn inspect(path: &Path) -> Result<WavInfo, String> {
    let mut r = open(path)?;
    let spec = r.spec();
    let frames = r.duration();
    let peak = norm_samples(&mut r).fold(0.0f32, |m, s| m.max(s.abs()));
    Ok(WavInfo { sample_rate: spec.sample_rate, channels: spec.channels, bits: spec.bits_per_sample, frames, peak })
}

/// out（n = 1）と、その別名 `<stem>-<n>.<ext>`（n ≥ 2）。
pub fn numbered(out: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        return out.to_path_buf();
    }
    let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match out.extension() {
        Some(ext) => format!("{stem}-{n}.{}", ext.to_string_lossy()),
        None => format!("{stem}-{n}"),
    };
    out.with_file_name(name)
}

/// 44.1k / 16bit / mono で書く。同じ dir の `.part` に書いて読み直し、out へ置く（既に在れば上書きせず `-2` … に置く）。
/// 失敗したら `.part` を消して何も残さない。戻り値は (置いた path, 中身)。
fn write_mono(out: &Path, samples: &[i16]) -> Result<(PathBuf, WavInfo), String> {
    let dir = out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
    let name = out.file_name().map(|s| s.to_string_lossy().into_owned()).ok_or_else(|| format!("The destination isn't a file name: {}", out.display()))?;
    // ponytail: Ctrl-C が書き出しの数 ms に当たると `.part` が残る（隠し file）。気になれば起動時に古い `.part-*` を掃除する
    let part = dir.join(format!(".{name}.part-{}", std::process::id()));
    let spec = hound::WavSpec { channels: CHANNELS, sample_rate: SAMPLE_RATE, bits_per_sample: BITS, sample_format: hound::SampleFormat::Int };
    let written = (|| {
        let mut w = hound::WavWriter::create(&part, spec)?;
        for &s in samples {
            w.write_sample(s)?;
        }
        w.finalize()
    })();
    let fail = |why: String| {
        let _ = std::fs::remove_file(&part);
        why
    };
    if let Err(e) = written {
        let full = matches!(&e, hound::Error::IoError(io) if io.raw_os_error() == Some(28));
        return Err(fail(if full {
            format!("Can't write {}: the disk ran out of space (nothing was left behind). Free some space, then record again", out.display())
        } else {
            format!("Can't write {}: {e} (nothing was left behind)", out.display())
        }));
    }
    let info = inspect(&part).map_err(|e| fail(format!("Can't read back the wav that was written (nothing was left behind): {e}")))?;
    if info.frames as usize != samples.len() {
        return Err(fail(format!("The length of the wav that was written doesn't match ({} / {} frames. Nothing was left behind)", info.frames, samples.len())));
    }
    for n in 1..=99 {
        let dest = numbered(out, n);
        // hard_link は dest が在れば失敗する = 他の file を上書きしない置き方
        match std::fs::hard_link(&part, &dest) {
            Ok(()) => {
                let _ = std::fs::remove_file(&part);
                return Ok((dest, info));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // hard link の無い file system（exFAT など）は rename。直前に在るかを見るだけなので、同時に作られると上書きしうる
            Err(_) if !dest.exists() => {
                return std::fs::rename(&part, &dest).map(|_| (dest, info)).map_err(|e| fail(format!("Can't put it in {}: {e}", out.display())));
            }
            Err(_) => continue,
        }
    }
    Err(fail(format!("Can't put it in {}: it and every alternate name from -2 to -99 already exist (nothing was left behind)", out.display())))
}
