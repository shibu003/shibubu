//! 鼻歌の録音・試聴と REAPER への insert（docs/ACCEPTANCE.md test3・v0.13 M6 の土台）。
//! `record` は mic → 44.1k / 16bit / mono の wav（録る前の検査・stop・device 外れ・上書きしない書き出し）、
//! `preview` は afplay での試聴と波形の peak 列、`reaper` は起動中の REAPER の選択 track の cursor 位置へその wav を入れる、
//! `surface` は REAPER を人が直接操作するための native handoff（検出・起動 / 前に出す・窓の確認・状態）、
//! `tempo` は録った wav の BPM の推定と「project の tempo に合わせますか」の提案の data（REAPER は編集しない）。
//! 分岐は docs/diagrams.md §10。CLI は src/bin/kangae-rec.rs と src/bin/kangae-surface.rs（`#[path]` で取り込むので main.rs と独立に compile できる）。

pub mod reaper;
pub mod record;
pub mod surface;

// ---- preview（旧 audio/preview.rs）----
pub mod preview {
    //! 録った wav の試聴と、波形の preview 用の peak 列（UI が描く data）。
    //! 試聴は macOS 同梱の `afplay`（/usr/bin/afplay）を spawn して child を持つだけ。止める = kill。
    //! `KANGAE_AFPLAY_BIN` で差し替え可（test は偽物を置く）。

    use super::record::{inspect, norm_samples, open};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::thread::JoinHandle;

    /// afplay の実行 file。`KANGAE_AFPLAY_BIN` で差し替え可。
    pub fn afplay_bin() -> PathBuf {
        std::env::var_os("KANGAE_AFPLAY_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/bin/afplay"))
    }

    /// 鳴っている 1 本。drop すると止まる（UI が欄を閉じても鳴り続けない）。
    pub struct Preview {
        child: Child,
        stderr: Option<JoinHandle<Vec<u8>>>,
    }

    impl Preview {
        /// wav を読めるか確かめてから afplay で鳴らし始める（鳴り終わりは待たない）。
        pub fn start(path: &Path) -> Result<Preview, String> {
            inspect(path)?;
            let bin = afplay_bin();
            let mut child = Command::new(&bin)
                .arg(path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("Can't start {} (preview needs afplay on macOS): {e}", bin.display()))?;
            let mut err = child.stderr.take();
            let stderr = std::thread::spawn(move || {
                let mut b = Vec::new();
                if let Some(r) = err.as_mut() {
                    let _ = r.read_to_end(&mut b);
                }
                b
            });
            Ok(Preview { child, stderr: Some(stderr) })
        }

        /// まだ鳴っているか
        pub fn is_playing(&mut self) -> bool {
            matches!(self.child.try_wait(), Ok(None))
        }

        /// 止める（鳴り終わっていれば何もしない）
        pub fn stop(&mut self) {
            if self.is_playing() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }

        /// 鳴り終わるまで待つ。afplay が失敗したら終わり方と stderr の 1 行
        pub fn wait(mut self) -> Result<(), String> {
            let status = self.child.wait().map_err(|e| format!("Can't wait for afplay: {e}"))?;
            let err = self.stderr.take().and_then(|h| h.join().ok()).unwrap_or_default();
            if status.success() {
                return Ok(());
            }
            let err = String::from_utf8_lossy(&err);
            let line = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
            Err(format!("afplay couldn't play it ({status}): {}", line.chars().take(300).collect::<String>()))
        }
    }

    impl Drop for Preview {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// 波形の preview 用: wav を bins 本に等分し、各本の (min, max) を -1.0〜1.0 で（channel は全部まとめて見る）。
    /// frame が bins より少なければ frame 数の本数。空の wav と bins = 0 は空。
    pub fn peaks(path: &Path, bins: usize) -> Result<Vec<(f32, f32)>, String> {
        let mut r = open(path)?;
        let ch = r.spec().channels.max(1) as usize;
        let frames = r.duration() as usize;
        let n = bins.min(frames);
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut out = vec![(f32::MAX, f32::MIN); n];
        for (k, s) in norm_samples(&mut r).enumerate() {
            let b = &mut out[((k / ch) * n / frames).min(n - 1)];
            *b = (b.0.min(s), b.1.max(s));
        }
        // 読めずに止まった後ろの本は無音扱い
        Ok(out.into_iter().map(|(lo, hi)| if lo > hi { (0.0, 0.0) } else { (lo, hi) }).collect())
    }

    /// peak 列を terminal 用の 1 行（▁〜█ の 8 段・-60〜0 dBFS）に。CLI の波形 preview。
    /// dB で刻むのは、小さい鼻歌（peak 0.03 ≒ -30 dB）も形が見え、無音は ▁ のままになるから。
    pub fn sparkline(peaks: &[(f32, f32)]) -> String {
        const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        peaks
            .iter()
            .map(|&(lo, hi)| {
                let db = 20.0 * lo.abs().max(hi.abs()).max(1e-9).log10();
                BARS[(((db + 60.0) / 60.0 * 7.0).round().clamp(0.0, 7.0)) as usize]
            })
            .collect()
    }
} // mod preview

// ---- tempo（旧 audio/tempo.rs）----
pub mod tempo {
    //! 録った鼻歌の BPM の推定と、「project の tempo に合わせますか」の提案の data（AI の提案の材料。REAPER は編集しない）。
    //! 推定: 10 ms ごとの音量（RMS を log で圧縮）→ その増え分（onset の強さ）→ 60〜200 BPM の間の自己相関の峰。
    //! 倍 / 半分の取り違えを減らすため、120 BPM を中心にした log 正規分布の重み（1 octave の幅）を掛ける。crate は足さない。

    use super::record::{norm_samples, open};
    use std::path::Path;

    /// 推定の下限（これより短い take は拍の繰り返しが足りない）
    pub const MIN_SECONDS: f32 = 4.0;
    const HOP_SECONDS: f64 = 0.01;
    const MIN_BPM: f64 = 60.0;
    const MAX_BPM: f64 = 200.0;
    /// 自己相関の峰 / 0 lag がこれ未満なら「拍が読めない」
    const MIN_CONFIDENCE: f32 = 0.1;
    /// 提案しない幅（再生速度の 2% 未満のずれは合っている扱い）
    const SAME_TEMPO: f64 = 0.02;

    /// 推定した tempo。
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Bpm {
        pub bpm: f32,
        /// 自己相関の峰の高さ（0 lag で割った値・0〜1）。大きいほど拍がはっきりしている
        pub confidence: f32,
    }

    /// wav の BPM を推定する（sample を溜めずに流して読む）。短すぎる・無音・拍が読めない時は None。
    pub fn estimate_bpm(path: &Path) -> Result<Option<Bpm>, String> {
        let mut r = open(path)?;
        let spec = r.spec();
        let ch = spec.channels.max(1) as usize;
        let mut it = norm_samples(&mut r);
        let mono = std::iter::from_fn(|| {
            let mut sum = 0.0;
            for _ in 0..ch {
                sum += it.next()?;
            }
            Some(sum / ch as f32)
        });
        Ok(estimate(mono, spec.sample_rate))
    }

    /// mono の samples（rate Hz）の BPM。
    pub fn estimate(mono: impl IntoIterator<Item = f32>, rate: u32) -> Option<Bpm> {
        let hop = ((rate as f64 * HOP_SECONDS).round() as usize).max(1);
        let (mut level, mut sum, mut n, mut frames, mut peak) = (Vec::new(), 0.0f64, 0usize, 0usize, 0.0f32);
        for s in mono {
            peak = peak.max(s.abs());
            sum += (s as f64).powi(2);
            n += 1;
            frames += 1;
            if n == hop {
                level.push((1.0 + 100.0 * (sum / n as f64).sqrt()).ln());
                (sum, n) = (0.0, 0);
            }
        }
        if rate == 0 || (frames as f32) < MIN_SECONDS * rate as f32 || peak < 0.01 {
            return None;
        }
        let onset: Vec<f64> = level.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
        let mean = onset.iter().sum::<f64>() / onset.len().max(1) as f64;
        let o: Vec<f64> = onset.iter().map(|x| x - mean).collect();
        let ac = |lag: usize| -> f64 {
            if lag >= o.len() {
                return 0.0;
            }
            o.iter().zip(&o[lag..]).map(|(a, b)| a * b).sum::<f64>() / (o.len() - lag) as f64
        };
        let zero = ac(0);
        if zero <= 0.0 {
            return None;
        }
        let lag_of = |bpm: f64| 60.0 / bpm / HOP_SECONDS;
        let (lo, hi) = (lag_of(MAX_BPM).floor() as usize, lag_of(MIN_BPM).ceil() as usize);
        let weight = |lag: f64| (-0.5 * (lag_of(120.0) / lag).log2().powi(2)).exp();
        let best = (lo.max(1)..=hi).max_by(|&a, &b| (ac(a) * weight(a as f64)).total_cmp(&(ac(b) * weight(b as f64))))?;
        // 峰の前後 3 点の放物線で lag を 1 hop より細かく
        let (a, b, c) = (ac(best - 1), ac(best), ac(best + 1));
        let d = a - 2.0 * b + c;
        let shift = if d < 0.0 { (0.5 * (a - c) / d).clamp(-0.5, 0.5) } else { 0.0 };
        let confidence = (b / zero) as f32;
        (confidence >= MIN_CONFIDENCE).then(|| Bpm { bpm: (60.0 / ((best as f64 + shift) * HOP_SECONDS)) as f32, confidence })
    }

    /// 「録音を project の tempo に合わせますか」の提案（KANGAE_MASTER_SPEC.md §7.2 の why / what_changes の形の一部）。
    /// data だけ。承認されたら take の再生速度（D_PLAYRATE）を playrate にする（音程は保つ）想定で、ここでは何も変えない。
    #[derive(Debug, Clone, PartialEq)]
    pub struct TempoProposal {
        /// 録音の推定 BPM（倍 / 半分に折った後。project の tempo に一番近い物）
        pub take_bpm: f32,
        /// project の tempo（REAPER の item の位置での実効 tempo）
        pub project_bpm: f64,
        /// 折った倍率（1 = そのまま / 2 = 倍のテンポとして数えた / 0.5 = 半分）
        pub fold: f32,
        /// 承認された時に take の再生速度（D_PLAYRATE）をこの値にする
        pub playrate: f64,
        /// 提案文（1 行）
        pub text: String,
        /// 根拠
        pub why: String,
        /// 何が変わるか（戻し方つき）
        pub what_changes: String,
    }

    /// 推定した take の tempo と project の tempo から提案を作る。ずれが 2% 未満（倍 / 半分も含めて）なら None（提案しない）。
    pub fn propose(take: Bpm, project_bpm: f64) -> Option<TempoProposal> {
        if !(project_bpm.is_finite() && project_bpm > 0.0 && take.bpm > 0.0) {
            return None;
        }
        let fold = [1.0f32, 2.0, 0.5]
            .into_iter()
            .min_by(|&a, &b| (project_bpm / (take.bpm * a) as f64).ln().abs().total_cmp(&(project_bpm / (take.bpm * b) as f64).ln().abs()))?;
        let folded = take.bpm * fold;
        let playrate = project_bpm / folded as f64;
        if (playrate - 1.0).abs() < SAME_TEMPO {
            return None;
        }
        let raw = take.bpm.round();
        let counted = match fold {
            f if f > 1.0 => format!(" (about {raw} BPM counted at double tempo, so about {} BPM)", folded.round()),
            f if f < 1.0 => format!(" (about {raw} BPM counted at half tempo, so about {} BPM)", folded.round()),
            _ => String::new(),
        };
        Some(TempoProposal {
            take_bpm: folded,
            project_bpm,
            fold,
            playrate,
            text: format!("Match the recording (about {} BPM) to the project's tempo ({} BPM)?", folded.round(), fmt_bpm(project_bpm)),
            why: format!(
                "Estimated from the gaps between rises in the recording's volume{counted} (confidence {:.2}). The project's tempo is the value read from REAPER",
                take.confidence
            ),
            what_changes: format!(
                "Plays this item at {playrate:.3} times the speed (the pitch stays). The project's tempo and the other items don't change. Undo in REAPER puts it back"
            ),
        })
    }

    /// 120 → "120"、92.5 → "92.5"
    fn fmt_bpm(bpm: f64) -> String {
        let s = format!("{bpm:.1}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    }
} // mod tempo
