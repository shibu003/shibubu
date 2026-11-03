//! Live Mirror PoC（v0.14 §3.3・§11 P2）。分岐は docs/diagrams.md §19。Native Direct（p-portal）とは独立。
//!   kangae-mirror                       PoC の窓: 窓を選ぶ → その窓だけの映像 → 映像の上の入力を元の app へ → 操作の種類ごとの可否
//!   kangae-mirror list                  capture できる窓（JSON）
//!   kangae-mirror measure textedit [--rounds N]   TextEdit で入力を種類ごとに送り、保存した file で確かめて数を JSON で出す
//!   kangae-mirror measure reaper [--rounds N]     起動中の REAPER で入力を送り、ReaScript の読み戻しで確かめる（終わったら元の値に戻す）
//! 映像・入力・意味の確認を別の物として持つ（v0.14 §2）。映像が見える事を「操作できる」とは書かない:
//! 「届いた」= 入力の後に元の app が描き直した frame が来た（意味の変化の証拠ではない）、「確かめた」= file / app の読み戻しが合った。
//! 入力は既定で CGEvent.postToPid（cursor を動かさない）。HID（本物の cursor）は helper に口があるが、この bin からは送らない。
//! helper は bridge/macos-mirror/KangaeMirror.swift。`KANGAE_MIRROR_HELPER` が無ければ、この実行 file の隣に swiftc で作る。
//! 口（W16M-0・src/surface/mirror.rs）: PoC の窓の人の入力は同意の後の `MirrorSurface::relay_human` だけ（relay の記録を書いてから送る）。
//! measure の入力は `AutomatedInput::send`（helper の input・relay の記録に書かない）。measure の stream は frame の時刻を取るだけで stdin に書かない。
//! feature `mirror` の時だけ build する（Cargo.toml の [[bin]]）。

#[allow(dead_code, unused_imports)]
#[path = "../surface/mod.rs"]
mod surface;
use surface::mirror::{consent_button_id, consent_prompt, helper_json, now_ms, relay_log_path, AutomatedInput, ConsentGate, MirrorFrame, MirrorInput, MirrorSurface, MirrorTarget, RelayLog};
use surface::ControlLease;

#[allow(dead_code, unused_imports)]
#[path = "../audio/mod.rs"]
mod audio;

use eframe::egui::{self, pos2, Color32, ColorImage, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ============================== helper ==============================

const HELPER_SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/bridge/macos-mirror/KangaeMirror.swift");

/// helper の実行 file。`KANGAE_MIRROR_HELPER` → この実行 file の隣（source より新しければ）→ swiftc で作る（混んだ機械では数分）
fn helper_path() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("KANGAE_MIRROR_HELPER") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().map_err(|e| format!("実行 file の場所が分かりません: {e}"))?;
    let bin = exe.with_file_name("kangae-mirror-helper");
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    if let (Some(b), Some(s)) = (mtime(&bin), mtime(Path::new(HELPER_SRC))) {
        if b >= s {
            return Ok(bin);
        }
    }
    let out = Command::new("/usr/bin/xcrun")
        .args(["swiftc", "-O", "-parse-as-library", HELPER_SRC, "-o"])
        .arg(&bin)
        .output()
        .map_err(|e| format!("swiftc を起こせません（Xcode の Command Line Tools が要ります）: {e}"))?;
    if !out.status.success() {
        return Err(format!("helper を build できません: {}", String::from_utf8_lossy(&out.stderr).lines().take(5).collect::<Vec<_>>().join(" / ")));
    }
    Ok(bin)
}

// ============================== stream ==============================

/// helper の stream の持ち主。PoC = 同意済みの `MirrorSurface`（人の口）・measure = 素の process（stdin は開けておくだけで書かない）
enum Owner {
    Surface(MirrorSurface),
    Raw(Child, #[allow(dead_code)] ChildStdin),
}

struct Stream {
    owner: Owner,
    latest: Arc<Mutex<Option<MirrorFrame>>>,
    /// 統計に残す frame ごとの（撮影, 受け取り）の時刻（ms・`MirrorFrame` の `meta.captured_ms` と `received_ms`）
    metas: Arc<Mutex<Vec<(f64, f64)>>>,
    events: Receiver<Value>,
    /// Surface の時だけ: `poll` が `MirrorSurface::stream` / `take_event` の物を events へ流す口（止まったら None）
    feed: Option<Sender<Value>>,
    window: u64,
}

impl Stream {
    fn start(helper: &Path, window: u64, max_width: u32, fps: u32) -> Result<Stream, String> {
        let mut child = Command::new(helper)
            .args(["stream", "--window", &window.to_string(), "--max-width", &max_width.to_string(), "--fps", &fps.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("helper を起こせません: {e}"))?;
        let stdin = child.stdin.take().ok_or("helper の stdin がありません")?;
        let stdout = child.stdout.take().ok_or("helper の stdout がありません")?;
        Ok(Stream::read(Owner::Raw(child, stdin), stdout, window))
    }

    /// 同意済みの Mirror（PoC の窓）。frame と出来事は `MirrorSurface` の読み手が持ち、`poll` で受け取る
    fn surface(m: MirrorSurface) -> Result<Stream, String> {
        let window = m.target().window_id;
        let (tx, events) = mpsc::channel();
        Ok(Stream { owner: Owner::Surface(m), latest: Arc::default(), metas: Arc::default(), events, feed: Some(tx), window })
    }

    /// Surface の時: 最新の frame を latest / metas へ、helper の出来事を events へ。止まったら stopped を 1 回だけ流す
    fn poll(&mut self) {
        let (Owner::Surface(m), Some(tx)) = (&mut self.owner, &self.feed) else { return };
        while let Some(ev) = m.take_event() {
            let _ = tx.send(ev);
        }
        match m.stream() {
            Ok(Some(f)) => keep(&self.latest, &self.metas, f),
            Ok(None) => {}
            Err(e) => {
                let reason = match e {
                    surface::mirror::MirrorError::WindowGone => "window_gone".to_string(),
                    e => e.to_string(),
                };
                let _ = tx.send(json!({"kind": "stopped", "reason": reason}));
                self.feed = None;
            }
        }
    }

    fn read(owner: Owner, mut stdout: impl Read + Send + 'static, window: u64) -> Stream {
        let latest: Arc<Mutex<Option<MirrorFrame>>> = Arc::default();
        let metas: Arc<Mutex<Vec<(f64, f64)>>> = Arc::default();
        let (tx, events) = mpsc::channel();
        let (l2, m2) = (Arc::clone(&latest), Arc::clone(&metas));
        std::thread::spawn(move || {
            while let Some((tag, payload)) = surface::mirror::read_message(&mut stdout) {
                match tag {
                    1 => {
                        if let Ok(mut f) = surface::mirror::parse_frame(payload) {
                            f.received_ms = now_ms();
                            keep(&l2, &m2, f);
                        }
                    }
                    2 => {
                        if let Ok(v) = serde_json::from_slice::<Value>(&payload) {
                            if tx.send(v).is_err() {
                                break;
                            }
                        }
                    }
                    _ => break,
                }
            }
            let _ = tx.send(json!({"kind": "stopped", "reason": "helper_exited"}));
        });
        Stream { owner, latest, metas, events, feed: None, window }
    }

    fn take_frame(&self) -> Option<MirrorFrame> {
        self.latest.lock().unwrap().take()
    }

    /// t_ms より後に撮った最初の frame の撮影の時刻
    fn frame_after(&self, t_ms: f64) -> Option<f64> {
        self.metas.lock().unwrap().iter().map(|(captured, _)| *captured).find(|c| *c > t_ms)
    }

    fn frames_between(&self, a: f64, b: f64) -> usize {
        self.metas.lock().unwrap().iter().filter(|(captured, _)| *captured > a && *captured <= b).count()
    }
}

/// 最新の frame を latest に・（撮影, 受け取り）の時刻を metas に（4000 を超えたら古い 2000 を捨てる）。Surface の poll と Raw の読み手の両方
fn keep(latest: &Mutex<Option<MirrorFrame>>, metas: &Mutex<Vec<(f64, f64)>>, f: MirrorFrame) {
    let mut m = metas.lock().unwrap();
    m.push((f.meta.captured_ms, f.received_ms));
    if m.len() > 4000 {
        m.drain(..2000);
    }
    drop(m);
    *latest.lock().unwrap() = Some(f);
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Owner::Raw(child, _) = &mut self.owner {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ============================== 操作の種類ごとの可否 ==============================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Click,
    Scroll,
    Drag,
    Key,
}
impl Kind {
    const ALL: [Kind; 4] = [Kind::Click, Kind::Scroll, Kind::Drag, Kind::Key];
    fn label(self) -> &'static str {
        match self {
            Kind::Click => "click",
            Kind::Scroll => "scroll",
            Kind::Drag => "drag",
            Kind::Key => "key",
        }
    }
}

/// 入力を送った後 この時間のうちに frame が来なければ「変化なし」
const CHANGE_WINDOW_MS: f64 = 1500.0;
/// 同じ種類で続けてこの回数 変化なしなら view_only
const VIEW_ONLY_AFTER: u32 = 2;

#[derive(Clone, Debug, PartialEq)]
enum Reach {
    /// まだ送っていない
    Unknown,
    /// 送った後に元の app が描き直した（意味の変化はまだ確かめていない）
    Reached { ms: f64 },
    /// 入力の前から画面が変わり続けていて、届いたかを映像では決められない
    Noisy,
    /// 続けて届かない
    ViewOnly,
    /// helper が送れなかった（窓が消えた等）
    Failed(String),
}

#[derive(Clone, Debug)]
struct KindState {
    reach: Reach,
    misses: u32,
    /// 送った時刻（helper の sent_ms）と、その前の 1.5 秒に frame が来ていたか
    pending: Option<(f64, bool)>,
}
impl Default for KindState {
    fn default() -> Self {
        Self { reach: Reach::Unknown, misses: 0, pending: None }
    }
}

/// 送った入力の結果を frame の到着で決める（UI の 1 frame ごとに呼ぶ）。frame_after(t) = t より後に撮った最初の frame の撮影の時刻
fn settle(state: &mut KindState, frame_after: impl Fn(f64) -> Option<f64>, now: f64) {
    let Some((sent, noisy)) = state.pending else { return };
    if let Some(captured) = frame_after(sent) {
        state.pending = None;
        if noisy {
            state.reach = Reach::Noisy;
        } else {
            state.misses = 0;
            state.reach = Reach::Reached { ms: captured - sent };
        }
    } else if now - sent > CHANGE_WINDOW_MS {
        state.pending = None;
        state.misses += 1;
        if state.misses >= VIEW_ONLY_AFTER {
            state.reach = Reach::ViewOnly;
        }
    }
}

// ============================== PoC の窓 ==============================

#[derive(Clone, Debug)]
struct Win {
    id: u64,
    pid: i64,
    bundle_id: String,
    app: String,
    title: String,
}

fn windows_of(v: &Value) -> Vec<Win> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            Some(Win {
                id: w["window_id"].as_u64()?,
                pid: w["pid"].as_i64()?,
                bundle_id: w["bundle_id"].as_str().unwrap_or("").to_string(),
                app: w["app"].as_str().unwrap_or("").to_string(),
                title: w["title"].as_str().unwrap_or("").to_string(),
            })
        })
        // 自分の窓と、題名の無い窓（常駐 app の小窓・panel）は選ばせない
        .filter(|w| w.pid as u32 != std::process::id() && !w.title.trim().is_empty())
        .collect()
}

enum Load<T> {
    Waiting(Receiver<Result<T, String>>),
    Ready(T),
    Failed(String),
}
impl<T> Load<T> {
    fn spawn(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Self
    where
        T: Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        Load::Waiting(rx)
    }
    fn poll(&mut self) {
        if let Load::Waiting(rx) = self {
            match rx.try_recv() {
                Ok(Ok(v)) => *self = Load::Ready(v),
                Ok(Err(e)) => *self = Load::Failed(e),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => *self = Load::Failed("中断しました".into()),
            }
        }
    }
}

struct Ready {
    helper: PathBuf,
    screen_capture: bool,
    windows: Vec<Win>,
}

fn prepare() -> Result<Ready, String> {
    let helper = helper_path()?;
    let status = helper_json(&helper, &["status"]).map_err(|e| e.to_string())?;
    let screen_capture = status["screen_capture"].as_bool().unwrap_or(false);
    // 別の Space の窓・tiling の window manager が画面の外へ退けた窓も選べる（ScreenCaptureKit は画面に出ていない窓も取れる。REAPER で実測）
    let windows = if screen_capture { windows_of(&helper_json(&helper, &["list", "--all"]).map_err(|e| e.to_string())?) } else { Vec::new() };
    Ok(Ready { helper, screen_capture, windows })
}

struct MirrorApp {
    ready: Load<Ready>,
    stream: Option<Stream>,
    texture: Option<TextureHandle>,
    frame_size: (usize, usize),
    content: [f64; 4],
    factor: f64,
    states: std::collections::HashMap<Kind, KindState>,
    stopped: Option<String>,
    frame_age_ms: Option<f64>,
    fps: f64,
    frames_seen: u64,
    fps_mark: (Instant, u64),
    helper_cpu: Option<f64>,
    drag_from: Option<(f64, f64)>,
    focused: bool,
    error: Option<String>,
    started_at: f64,
    /// app ごとの同意（PoC は DB を持たないので process の中だけ。Page の配線では ui_state("mirror-consent", bundle id)）
    gate: ConsentGate,
    /// 同意の 1 行を出している窓（押すまで helper の stream を起こさない）
    asking: Option<Win>,
    lease: ControlLease,
}

impl Win {
    fn target(&self) -> MirrorTarget {
        MirrorTarget { window_id: self.id, pid: self.pid, bundle_id: self.bundle_id.clone(), app_name: self.app.clone() }
    }
}

impl MirrorApp {
    fn new() -> Self {
        Self {
            ready: Load::spawn(prepare),
            stream: None,
            texture: None,
            frame_size: (0, 0),
            content: [0.0; 4],
            factor: 1.0,
            states: Kind::ALL.iter().map(|k| (*k, KindState::default())).collect(),
            stopped: None,
            frame_age_ms: None,
            fps: 0.0,
            frames_seen: 0,
            fps_mark: (Instant::now(), 0),
            helper_cpu: None,
            drag_from: None,
            focused: false,
            error: None,
            started_at: 0.0,
            gate: ConsentGate::default(),
            asking: None,
            lease: ControlLease::human(now_ms()),
        }
    }

    fn open(&mut self, helper: &Path, w: &Win) {
        self.stream = None;
        self.texture = None;
        self.stopped = None;
        self.error = None;
        self.frame_age_ms = None;
        self.states = Kind::ALL.iter().map(|k| (*k, KindState::default())).collect();
        self.asking = None;
        let target = w.target();
        let grant = match self.gate.check(&target) {
            Ok(g) => g,
            Err(_) => {
                self.asking = Some(w.clone());
                return;
            }
        };
        let opened = RelayLog::open(&relay_log_path())
            .and_then(|log| MirrorSurface::open(helper, target, grant, log))
            .map_err(|e| e.to_string())
            .and_then(Stream::surface);
        match opened {
            Ok(s) => {
                self.stream = Some(s);
                self.started_at = now_ms();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// 入力を送り、その種類の結果待ちに入る（前の 1.5 秒に frame が来ていたら、映像では判定しない）
    fn input(&mut self, kind: Kind, cmd: Value) {
        let Some(s) = self.stream.as_mut() else { return };
        if self.stopped.is_some() {
            return;
        }
        let Owner::Surface(m) = &mut s.owner else { return };
        let sent = serde_json::from_value::<MirrorInput>(cmd).map_err(|e| e.to_string()).and_then(|i| m.relay_human(&mut self.lease, i).map_err(|e| e.to_string()));
        if let Err(e) = sent {
            self.states.get_mut(&kind).unwrap().reach = Reach::Failed(e);
            return;
        }
        let now = now_ms();
        let noisy = s.frames_between(now - CHANGE_WINDOW_MS, now) > 0;
        // sent_ms は helper の答えで上書きする（下の events の処理）
        self.states.get_mut(&kind).unwrap().pending = Some((now, noisy));
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(s) = self.stream.as_mut() {
            s.poll();
        }
        let Some(s) = self.stream.as_ref() else { return };
        while let Ok(ev) = s.events.try_recv() {
            match ev["kind"].as_str() {
                Some("stopped") => self.stopped = Some(ev["reason"].as_str().unwrap_or("").to_string()),
                Some("stats") => self.helper_cpu = ev["cpu_pct"].as_f64(),
                Some("sent") => {
                    let kind = match ev["op"].as_str() {
                        Some("click") => Kind::Click,
                        Some("scroll") => Kind::Scroll,
                        Some("drag") => Kind::Drag,
                        _ => Kind::Key,
                    };
                    if let (Some(st), Some(sent)) = (self.states.get_mut(&kind), ev["sent_ms"].as_f64()) {
                        if let Some(p) = st.pending.as_mut() {
                            p.0 = sent;
                        }
                    }
                }
                Some("error") => {
                    let reason = ev["reason"].as_str().unwrap_or("").to_string();
                    if reason == "window_gone" || reason == "pid_changed" {
                        self.stopped = Some(reason.clone());
                    }
                    for st in self.states.values_mut().filter(|st| st.pending.is_some()) {
                        st.pending = None;
                        st.reach = Reach::Failed(reason.clone());
                    }
                }
                _ => {}
            }
        }
        if let Some(f) = s.take_frame() {
            self.frame_age_ms = Some(f.received_ms - f.meta.captured_ms);
            self.frame_size = (f.meta.width as usize, f.meta.height as usize);
            self.content = f.meta.content;
            self.factor = f.meta.factor;
            self.frames_seen = f.meta.seq;
            let img = ColorImage::from_rgba_unmultiplied([self.frame_size.0, self.frame_size.1], &f.rgba);
            match self.texture.as_mut() {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("mirror", img, TextureOptions::LINEAR)),
            }
        }
        let now = now_ms();
        for st in self.states.values_mut() {
            settle(st, |t| s.frame_after(t), now);
        }
        let (t0, n0) = self.fps_mark;
        if t0.elapsed() >= Duration::from_secs(1) {
            self.fps = (self.frames_seen.saturating_sub(n0)) as f64 / t0.elapsed().as_secs_f64();
            self.fps_mark = (Instant::now(), self.frames_seen);
        }
    }
}

/// OS の「視差効果を減らす」（または KANGAE_REDUCE_MOTION=1）。起動時に 1 回だけ読む
fn reduce_motion() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("KANGAE_REDUCE_MOTION").map(|v| v == "1").unwrap_or(false)
            || Command::new("/usr/bin/defaults").args(["read", "com.apple.universalaccess", "reduceMotion"]).output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1").unwrap_or(false)
    })
}

/// 灰色の帯 + 左から右へ 1.2 秒で流れる明るい帯（DESIGN §15.2-2）。reduce motion なら流さない
fn skeleton(ui: &mut egui::Ui, rect: Rect) {
    let p = ui.painter_at(rect);
    let base = if ui.visuals().dark_mode { Color32::from_gray(52) } else { Color32::from_gray(228) };
    let shine = if ui.visuals().dark_mode { Color32::from_gray(66) } else { Color32::from_gray(240) };
    p.rect_filled(rect, 8.0, base);
    if !reduce_motion() {
        let t = (ui.input(|i| i.time) % 1.2) / 1.2;
        let x = rect.left() - 240.0 + (rect.width() + 480.0) * t as f32;
        p.rect_filled(Rect::from_min_size(pos2(x, rect.top()), Vec2::new(240.0, rect.height())).intersect(rect), 8.0, shine);
        ui.ctx().request_repaint();
    }
}

fn reach_text(r: &Reach) -> String {
    match r {
        Reach::Unknown => "未確認".into(),
        Reach::Reached { ms } => format!("届いた（{ms:.0} ms で画面が変化・意味は未確認）"),
        Reach::Noisy => "画面が常に変わっていて判定できない".into(),
        Reach::ViewOnly => "view_only（届かない。元の app で操作してください）".into(),
        Reach::Failed(e) => format!("送れない（{e}）"),
    }
}

impl eframe::App for MirrorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ready.poll();
        self.pump(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
        let mut open: Option<Win> = None;
        let mut request = false;
        let mut refresh = false;
        // 窓が細い時（tiling の window manager が細い列に置く）も映像の場所が残るよう、一覧は幅の 35% まで
        let list_w = (ctx.screen_rect().width() * 0.35).clamp(150.0, 260.0);
        egui::SidePanel::left("windows").resizable(false).exact_width(list_w).show(ctx, |ui| {
            ui.add_space(8.0);
            match &self.ready {
                Load::Waiting(_) => {
                    for _ in 0..8 {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
                        skeleton(ui, r.shrink2(Vec2::new(0.0, 6.0)));
                    }
                }
                Load::Failed(e) => {
                    ui.label(e);
                    if ui.button("もう一度").clicked() {
                        refresh = true;
                    }
                }
                Load::Ready(r) if !r.screen_capture => {
                    ui.label("画面収録が未許可です。選んだ窓 1 つの映像だけを取り、窓を閉じると止まります。");
                    if ui.button("許可を求める").clicked() {
                        request = true;
                    }
                    if ui.button("確認し直す").clicked() {
                        refresh = true;
                    }
                }
                Load::Ready(r) => {
                    if r.windows.is_empty() {
                        ui.label("capture できる窓がありません");
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for w in &r.windows {
                            let chosen = self.stream.as_ref().is_some_and(|s| s.window == w.id);
                            let text = if w.title.is_empty() { w.app.clone() } else { format!("{} — {}", w.app, w.title) };
                            if ui.selectable_label(chosen, text).clicked() {
                                open = Some(w.clone());
                            }
                        }
                    });
                    if ui.button("一覧を更新").clicked() {
                        refresh = true;
                    }
                }
            }
        });
        if request {
            if let Load::Ready(r) = &self.ready {
                let _ = helper_json(&r.helper, &["request"]);
            }
            refresh = true;
        }
        if refresh {
            self.ready = Load::spawn(prepare);
        }
        let helper = match &self.ready {
            Load::Ready(r) => Some(r.helper.clone()),
            _ => None,
        };
        if let (Some(w), Some(h)) = (open, helper) {
            self.open(&h, &w);
        }
        egui::TopBottomPanel::bottom("caps").show(ctx, |ui| {
            if self.stream.is_none() {
                return;
            }
            egui::Grid::new("caps_grid").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                let label = match (&self.stopped, &self.texture) {
                    (Some(_), _) => "stale",
                    (None, Some(_)) => "visual_live",
                    (None, None) => "—",
                };
                ui.label("同期");
                ui.label(format!("{label}（映像のみ。意味の状態は unsupported）"));
                ui.end_row();
                for k in Kind::ALL {
                    ui.label(k.label());
                    ui.label(reach_text(&self.states[&k].reach));
                    ui.end_row();
                }
                ui.label("frame");
                ui.label(format!(
                    "{:.0} ms 前・{:.1} fps・helper の CPU {}",
                    self.frame_age_ms.unwrap_or(f64::NAN),
                    self.fps,
                    self.helper_cpu.map(|c| format!("{c:.0}%")).unwrap_or("—".into())
                ));
                ui.end_row();
            });
        });
        let mut granted: Option<Win> = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(w) = &self.asking {
                ui.label(consent_prompt(&w.app));
                let b = ui.button("映す");
                ctx.accesskit_node_builder(b.id, |n| n.set_author_id(consent_button_id(&w.bundle_id)));
                if b.clicked() {
                    granted = Some(w.clone());
                }
                return;
            }
            if let Some(e) = &self.error {
                ui.label(e);
                return;
            }
            if let Some(reason) = &self.stopped {
                ui.label(match reason.as_str() {
                    "window_gone" | "pid_changed" => "元の窓が閉じました。左の一覧から選び直してください",
                    "no_screen_capture_permission" => "画面収録が未許可です",
                    _ => "映像が止まりました。左の一覧から選び直してください",
                });
            }
            if self.stream.is_none() {
                return;
            }
            let avail = ui.available_size();
            let Some(tex) = self.texture.clone() else {
                let (r, _) = ui.allocate_exact_size(avail, Sense::hover());
                skeleton(ui, r);
                return;
            };
            let (fw, fh) = (self.frame_size.0 as f32, self.frame_size.1 as f32);
            let k = (avail.x / fw).min(avail.y / fh).min(1.0);
            let size = Vec2::new(fw * k, fh * k);
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            ui.painter().image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            let meta = surface::MirrorFrameMeta { seq: 0, width: self.frame_size.0 as u32, height: self.frame_size.1 as u32, captured_ms: 0.0, content: self.content, factor: self.factor };
            let map = |p: egui::Pos2| surface::mirror::frame_point_to_window(&meta, ((p.x - rect.left()) / k) as f64, ((p.y - rect.top()) / k) as f64).map(|[x, y]| (x, y));
            if resp.clicked() {
                self.focused = true;
                if let Some((x, y)) = resp.interact_pointer_pos().and_then(map) {
                    self.input(Kind::Click, json!({"op": "click", "x": x, "y": y, "count": 1}));
                }
            }
            if resp.double_clicked() {
                if let Some((x, y)) = resp.interact_pointer_pos().and_then(map) {
                    self.input(Kind::Click, json!({"op": "click", "x": x, "y": y, "count": 2}));
                }
            }
            if resp.drag_started() {
                self.drag_from = resp.interact_pointer_pos().and_then(map);
            }
            if resp.drag_stopped() {
                if let (Some((x, y)), Some((x2, y2))) = (self.drag_from.take(), resp.interact_pointer_pos().and_then(map)) {
                    self.input(Kind::Drag, json!({"op": "drag", "x": x, "y": y, "x2": x2, "y2": y2, "steps": 8}));
                }
            }
            if resp.hovered() {
                let d = ui.input(|i| i.raw_scroll_delta);
                if d != Vec2::ZERO {
                    if let Some((x, y)) = ui.input(|i| i.pointer.hover_pos()).and_then(map) {
                        self.input(Kind::Scroll, json!({"op": "scroll", "x": x, "y": y, "dy": d.y.round() as i64, "dx": d.x.round() as i64}));
                    }
                }
            }
            if ui.input(|i| i.pointer.any_click()) && !resp.hovered() {
                self.focused = false;
            }
            if self.focused {
                let events = ui.input(|i| i.events.clone());
                for e in events {
                    match e {
                        egui::Event::Text(t) => self.input(Kind::Key, json!({"op": "text", "text": t})),
                        egui::Event::Key { key, pressed: true, modifiers, .. } => {
                            let name = match key {
                                egui::Key::Enter => "return",
                                egui::Key::Backspace => "delete",
                                egui::Key::Tab => "tab",
                                egui::Key::Escape => "escape",
                                egui::Key::ArrowLeft => "left",
                                egui::Key::ArrowRight => "right",
                                egui::Key::ArrowUp => "up",
                                egui::Key::ArrowDown => "down",
                                k if modifiers.command => match k.name().to_lowercase().as_str() {
                                    n @ ("a" | "c" | "v" | "x" | "z" | "s") => {
                                        self.input(Kind::Key, json!({"op": "key", "key": n, "mods": ["cmd"]}));
                                        continue;
                                    }
                                    _ => continue,
                                },
                                _ => continue,
                            };
                            self.input(Kind::Key, json!({"op": "key", "key": name, "mods": []}));
                        }
                        _ => {}
                    }
                }
                ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(2.0, ui.visuals().selection.bg_fill), egui::StrokeKind::Outside);
            }
        });
        // 人が「映す」を押した時だけ同意を足して開く
        if let (Some(w), Load::Ready(r)) = (granted, &self.ready) {
            self.gate.grant(&w.target(), now_ms());
            let helper = r.helper.clone();
            self.open(&helper, &w);
        }
    }
}

// ============================== 測る（UI 無し） ==============================

/// 今 前にある app の名前（LaunchServices の lsappinfo。Apple Event を使わないので、混んで答えの遅い app を経由しない）。
/// CGEvent が元の app に届くかは「元の app が前か」で変わるので、試すたびに前後で記録する
fn frontmost() -> String {
    // `lsappinfo visibleProcessList` は前から順: ASN:0x0-0xc00c-"Orca": ASN:0x0-0x19c19c-"TextEdit": …（名前の空白は _）
    let out = Command::new("/usr/bin/lsappinfo").arg("visibleProcessList").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    out.split('"').nth(1).unwrap_or("").replace('_', " ")
}

/// 1 回の試しの結果: 入力の後に撮った最初の frame までの ms・その前から画面が動いていたか・読み戻しの判定・読み戻しの中身。
/// verified は Some(true) = 読み戻しで変化を確かめた / Some(false) = 読めたが期限まで変化しなかった / None = 読み戻し自体ができなかった
/// （混んだ機械では Accessibility が kAXErrorCannotComplete を返す。届かなかったとは数えない）
struct Outcome {
    visible: Option<f64>,
    noisy: bool,
    verified: Option<bool>,
    detail: String,
}

/// 読み戻しを待つ期限（混んだ機械では元の app が入力を処理するまで秒単位で遅れる）
const VERIFY_WAIT: Duration = Duration::from_secs(8);

struct Measure {
    helper: PathBuf,
    stream: Stream,
    rows: Vec<Value>,
    app: &'static str,
    pid: i64,
    title: String,
}

impl Measure {
    /// 入力を 1 つ送り、helper の sent を待つ。返すのは 送った時刻・その後の最初の frame までの ms・送る前 1.5 秒に frame が来ていたか
    /// 自動の口（`AutomatedInput`・helper の input）で送る。stream は frame の時刻を取るだけ
    fn send(&mut self, cmd: Value) -> Result<(Option<f64>, bool), String> {
        while let Ok(ev) = self.stream.events.try_recv() {
            if ev["kind"] == "stopped" {
                return Err(format!("映像が止まりました: {}", ev["reason"]));
            }
        }
        let input = serde_json::from_value::<MirrorInput>(cmd).map_err(|e| format!("入力の形が違います: {e}"))?;
        let target = MirrorTarget { window_id: self.stream.window, pid: self.pid, bundle_id: String::new(), app_name: self.app.into() };
        let sent = AutomatedInput::new(self.helper.clone()).send(&target, &input).map_err(|e| e.to_string())?;
        let noisy = self.stream.frames_between(sent - CHANGE_WINDOW_MS, sent) > 0;
        let until = Instant::now() + Duration::from_millis(CHANGE_WINDOW_MS as u64);
        let visible = loop {
            if let Some(captured) = self.stream.frame_after(sent) {
                break Some(captured - sent);
            }
            if Instant::now() > until {
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Ok((visible, noisy))
    }

    /// 1 回の試し。前の app を前後で取り、途中で替わった試しは valid = false（他の session や window manager が前を替えた）
    fn trial(&mut self, kind: &str, round: usize, f: impl FnOnce(&mut Measure) -> Result<Outcome, String>) {
        let before = frontmost();
        let r = f(self);
        let after = frontmost();
        let state = if before == self.app { "source_active" } else { "source_inactive" };
        let row = match r {
            Ok(o) => json!({
                "app": self.app, "kind": kind, "round": round, "state": state, "front_before": before, "front_after": after,
                "valid": before == after && o.verified.is_some(), "input_to_visible_change_ms": o.visible, "visual_evidence": o.visible.is_some() && !o.noisy,
                "verified": o.verified, "detail": o.detail,
            }),
            Err(e) => json!({"app": self.app, "kind": kind, "round": round, "state": state, "front_before": before, "front_after": after,
                             "valid": false, "verified": null, "error": e}),
        };
        // 1 つ終えるたびに stderr へ（混んだ機械では 1 回の測定が 40 分かかる。途中で止まっても失わない）
        eprintln!("{row}");
        self.rows.push(row);
    }

    fn text(&self) -> Option<String> {
        helper_json(&self.helper, &["ax-text", "--pid", &self.pid.to_string(), "--title", &self.title]).ok()?["text"].as_str().map(str::to_string)
    }

    /// 文書の中身が ok になるまで VERIFY_WAIT 待つ。返すのは 判定（読めなければ None）と最後に読めた中身
    fn wait_text(&self, ok: impl Fn(&str) -> bool) -> (Option<bool>, String) {
        // AX の 1 回が混雑で 10 秒かかる事があるので、読むのは 2 秒後と期限の 2 回だけ
        let mut last: Option<String> = None;
        for wait in [Duration::from_secs(2), VERIFY_WAIT - Duration::from_secs(2)] {
            std::thread::sleep(wait);
            if let Some(x) = self.text() {
                if ok(&x) {
                    return (Some(true), x);
                }
                last = Some(x);
            }
        }
        (last.as_ref().map(|_| false), last.unwrap_or_default())
    }

    /// 文書の loc 文字目の矩形（窓の中の割合）。x は左端、y は縦の真ん中
    fn char_at(&self, loc: usize) -> Result<(f64, f64, f64), String> {
        let v = helper_json(&self.helper, &["ax-bounds", "--pid", &self.pid.to_string(), "--window", &self.stream.window.to_string(), "--loc", &loc.to_string(), "--len", "1", "--title", &self.title]).map_err(|e| e.to_string())?;
        let r = &v["rect"];
        let x = r["x"].as_f64().ok_or_else(|| format!("ax-bounds が矩形を返しません（{}）", v["reason"]))?;
        Ok((x, r["y"].as_f64().unwrap_or(0.0) + r["h"].as_f64().unwrap_or(0.0) / 2.0, r["w"].as_f64().unwrap_or(0.0)))
    }
}

fn wait_window(helper: &Path, pid: i64, title_has: &str, timeout: Duration) -> Result<Win, String> {
    let t = Instant::now();
    loop {
        let ws = windows_of(&helper_json(helper, &["list", "--all"]).map_err(|e| e.to_string())?);
        if let Some(w) = ws.into_iter().find(|w| w.pid == pid && (title_has.is_empty() || w.title.contains(title_has))) {
            return Ok(w);
        }
        if t.elapsed() > timeout {
            return Err(format!("pid {pid} の窓（題名に {title_has:?}）が {} 秒で出ません", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn pid_of(process: &str) -> Option<i64> {
    audio::surface::pid_of(process).map(|p| p as i64)
}

fn load_avg() -> String {
    Command::new("/usr/sbin/sysctl").args(["-n", "vm.loadavg"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// 受け取った frame の 受け取り − 撮影（frame_age_ms）と、fps
fn frame_stats(stream: &Stream, from: f64) -> Value {
    let m = stream.metas.lock().unwrap();
    let mut ages: Vec<f64> = m.iter().filter(|(_, received)| *received >= from).map(|(captured, received)| received - captured).collect();
    ages.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| ages.get(((ages.len() as f64 - 1.0) * p).round() as usize).map(|v| v.round());
    let span = (now_ms() - from) / 1000.0;
    json!({"samples": ages.len(), "p50": pct(0.5), "p95": pct(0.95), "max": ages.last().map(|v| v.round()), "fps": ages.len() as f64 / span.max(0.001)})
}

/// 種類 × 状態ごとの input_success_rate（valid な試しだけ）と、映像の証拠が取れた数
fn summarize(rows: &[Value]) -> Value {
    let mut out = serde_json::Map::new();
    for r in rows.iter().filter(|r| r["valid"] == true) {
        let key = format!("{}/{}/{}", r["app"].as_str().unwrap_or(""), r["state"].as_str().unwrap_or(""), r["kind"].as_str().unwrap_or(""));
        let e = out.entry(key).or_insert_with(|| json!({"n": 0, "verified": 0, "visual_evidence": 0, "visible_ms": []}));
        e["n"] = json!(e["n"].as_u64().unwrap() + 1);
        if r["verified"] == true {
            e["verified"] = json!(e["verified"].as_u64().unwrap() + 1);
        }
        if r["visual_evidence"] == true {
            e["visual_evidence"] = json!(e["visual_evidence"].as_u64().unwrap() + 1);
        }
        if let Some(ms) = r["input_to_visible_change_ms"].as_f64() {
            e["visible_ms"].as_array_mut().unwrap().push(json!(ms.round()));
        }
    }
    for v in out.values_mut() {
        v["input_success_rate"] = json!(v["verified"].as_f64().unwrap() / v["n"].as_f64().unwrap());
    }
    // 数えなかった試し（前の app が途中で替わった・読み戻しができなかった・helper の失敗）
    out.insert("not_counted".into(), json!(rows.iter().filter(|r| r["valid"] != true).count()));
    Value::Object(out)
}

/// TextEdit の 1 周: key（確定した文字）・cmd+S・cmd+Z・click（AX で引いた文字の前）・drag（1 語を選んで置き換え）・確定した日本語。
/// 届いたかは Accessibility の文書の中身（保存していない編集も含む）で、保存は disk の file で、別々に確かめる
/// 短い文書の中身。周ごと・段ごとに別の語を置く（click は gamma{tag}{r} の前・drag は delta{tag}{r} を選ぶ）。
/// 後ろの段では cmd+Z が届かず文書が戻らないので、前の周の結果に次の周が引きずられないようにする
fn short_doc(rounds: usize) -> String {
    let mut s = String::from("alpha beta\n");
    for tag in ["i", "a"] {
        for r in 1..=rounds {
            s += &format!("gamma{tag}{r} delta{tag}{r} epsilon\n");
        }
    }
    s
}

/// TextEdit の 1 周: key（確定した文字）・cmd+S・cmd+Z・click（AX で引いた文字の前）・drag（1 語を選んで置き換え）・確定した日本語。
/// どれも周ごとに固有の印を入れ、「印が 1 つ増えた・期待の場所に在る」で判定する（caret の場所に依らない）。
/// 届いたかは Accessibility の文書の中身（保存していない編集も含む）で、保存は disk の file で、別々に確かめる
fn textedit_round(m: &mut Measure, round: usize, tag: &str, file: &Path) {
    let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    let count = |t: &str, mark: &str| t.matches(mark).count();
    let typed = |m: &mut Measure, mark: String| -> Result<Outcome, String> {
        let before = m.text().ok_or("文書を読めません（Accessibility）")?;
        let (visible, noisy) = m.send(json!({"op": "text", "text": mark}))?;
        let n = count(&before, &mark);
        let (verified, after) = m.wait_text(|t| count(t, &mark) == n + 1 && t.chars().count() == before.chars().count() + mark.chars().count());
        Ok(Outcome { visible, noisy, verified, detail: format!("{mark:?} が {} 個 → {} 個", n, count(&after, &mark)) })
    };
    m.trial("key", round, |m| typed(m, format!("K{tag}{round}")));
    m.trial("shortcut_save", round, |m| {
        let doc = m.text().ok_or("文書を読めません（Accessibility）")?;
        let before = read(file);
        let (visible, noisy) = m.send(json!({"op": "key", "key": "s", "mods": ["cmd"]}))?;
        let t = Instant::now();
        while read(file) != doc && t.elapsed() < VERIFY_WAIT {
            std::thread::sleep(Duration::from_millis(300));
        }
        let disk = read(file);
        // 保存する変更が無い時は、disk と文書が同じでも保存の証拠にならない → 判定しない
        let verified = (doc != before).then_some(disk == doc);
        Ok(Outcome { visible, noisy, verified, detail: format!("保存する変更 {}・disk が文書と同じ {}", doc != before, disk == doc) })
    });
    m.trial("shortcut_undo", round, |m| {
        // 固有の印を入れ、それが入ったのを見てから cmd+Z → 印が消えたか（印が入らなければ判定しない）
        let mark = format!("U{tag}{round}");
        m.send(json!({"op": "text", "text": mark}))?;
        let (typed, _) = m.wait_text(|t| t.contains(&mark));
        if typed != Some(true) {
            return Err(format!("{mark} が入らないので undo を判定しません"));
        }
        let (visible, noisy) = m.send(json!({"op": "key", "key": "z", "mods": ["cmd"]}))?;
        let (verified, _) = m.wait_text(|t| !t.contains(&mark));
        Ok(Outcome { visible, noisy, verified, detail: format!("{mark} を入れてから cmd+Z") })
    });
    m.trial("click", round, |m| {
        let text = m.text().ok_or("文書を読めません（Accessibility）")?;
        let word = format!("gamma{tag}{round}");
        let at = text.find(&word).ok_or(format!("文書に {word} がありません"))?;
        let (x, y, w) = m.char_at(text[..at].chars().count())?;
        let (visible, noisy) = m.send(json!({"op": "click", "x": x + w * 0.1, "y": y, "count": 1}))?;
        let mark = format!("C{tag}{round}");
        m.send(json!({"op": "text", "text": mark}))?;
        let target = format!("{mark}{word}");
        let (verified, after) = m.wait_text(|t| t.contains(&target));
        Ok(Outcome { visible, noisy, verified, detail: format!("{mark} が {word} の前に: {:?}（入った所 {:?}）", verified, after.find(&mark)) })
    });
    m.trial("drag", round, |m| {
        let text = m.text().ok_or("文書を読めません（Accessibility）")?;
        let word = format!("delta{tag}{round}");
        let at = text.find(&word).ok_or(format!("文書に {word} がありません"))?;
        let loc = text[..at].chars().count();
        let last = loc + word.chars().count() - 1;
        let (x0, y0, _) = m.char_at(loc)?;
        let (xl, _, wl) = m.char_at(last)?;
        let (visible, noisy) = m.send(json!({"op": "drag", "x": x0 + 0.0005, "y": y0, "x2": xl + wl * 0.9, "y2": y0, "steps": 10}))?;
        let mark = format!("D{tag}{round}");
        m.send(json!({"op": "text", "text": mark}))?;
        let (verified, _) = m.wait_text(|t| t.contains(&format!(" {mark} ")) && !t.contains(&word));
        Ok(Outcome { visible, noisy, verified, detail: format!("{word} を {mark} に置き換え: {verified:?}") })
    });
    m.trial("text_japanese", round, |m| typed(m, format!("日本{tag}{round}")));
}

fn textedit_scroll_round(m: &mut Measure, round: usize) {
    m.trial("scroll", round, |m| {
        // 1 行目の文字の縦位置（窓の中の割合）。scroll すると動く。題名でこの文書の窓を引く（focused の窓は別の文書かもしれない）
        let ax = |m: &Measure| -> Option<f64> { m.char_at(0).ok().map(|(_, y, _)| y) };
        let before = ax(m).ok_or("1 行目の位置を読めません（Accessibility）")?;
        let dy = if before < 0.0 { 800 } else { -800 };
        let (visible, noisy) = m.send(json!({"op": "scroll", "x": 0.5, "y": 0.5, "dy": dy, "dx": 0}))?;
        let t = Instant::now();
        let mut after = None;
        while t.elapsed() < VERIFY_WAIT {
            after = ax(m).or(after);
            if after.is_some_and(|a: f64| (a - before).abs() > 0.001) {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        let verified = after.map(|a| (a - before).abs() > 0.001);
        Ok(Outcome { visible, noisy, verified, detail: format!("1 行目の縦位置 {before:.3} → {after:?}") })
    });
}

fn open_textedit(path: &Path, background: bool) -> Result<(), String> {
    let mut c = Command::new("/usr/bin/open");
    if background {
        c.arg("-g"); // 前の app を替えない（focus を奪わない）
    }
    c.args(["-a", "TextEdit"]).arg(path).status().map_err(|e| e.to_string())?;
    Ok(())
}

/// TextEdit: (1) 元の app が後ろ（Kangae が前の時と同じ）で 2 つの文書に入力 → (2) 元の app を前に出して（focus を奪う）同じ事。
/// どちらも postToPid。前の app は試すたびに記録し、終わったら元の前の app に戻す
fn measure_textedit(helper: &Path, rows: &mut Vec<Value>, rounds: usize, dir: &Path, frames: &mut Value) -> Result<(), String> {
    let short = dir.join("mirror-short.txt");
    let long = dir.join("mirror-long.txt");
    std::fs::write(&short, short_doc(rounds)).map_err(|e| e.to_string())?;
    std::fs::write(&long, (1..=300).map(|i| format!("line {i}\n")).collect::<String>()).map_err(|e| e.to_string())?;
    let original_front = frontmost();
    // long を先・short を後に開く（後に開いた short が TextEdit の中の key window になる。key は key window にしか届かない）
    open_textedit(&long, true)?;
    let t = Instant::now();
    let pid = loop {
        if let Some(p) = pid_of("TextEdit") {
            break p;
        }
        if t.elapsed() > Duration::from_secs(30) {
            return Err("TextEdit が起動しません".into());
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    open_textedit(&short, true)?;
    let ws = wait_window(helper, pid, "mirror-short", Duration::from_secs(60))?;
    let wl = wait_window(helper, pid, "mirror-long", Duration::from_secs(60))?;
    let run = |rows: &mut Vec<Value>, frames: &mut Value, phase: &str, tag: &str, rounds: usize| -> Result<(), String> {
        let started = now_ms();
        let mut m = Measure { helper: helper.to_path_buf(), stream: Stream::start(helper, ws.id, 960, 15)?, rows: Vec::new(), app: "TextEdit", pid, title: ws.title.clone() };
        std::thread::sleep(Duration::from_millis(2500));
        for r in 1..=rounds {
            textedit_round(&mut m, r, tag, &short);
        }
        frames[phase] = frame_stats(&m.stream, started);
        rows.append(&mut m.rows);
        drop(m);
        let mut m = Measure { helper: helper.to_path_buf(), stream: Stream::start(helper, wl.id, 960, 15)?, rows: Vec::new(), app: "TextEdit", pid, title: wl.title.clone() };
        std::thread::sleep(Duration::from_millis(2500));
        for r in 1..=rounds {
            textedit_scroll_round(&mut m, r);
        }
        rows.append(&mut m.rows);
        Ok(())
    };
    // (1) 後ろ
    let inactive = run(rows, frames, "source_inactive", "i", rounds);
    // (2) 前に出す（PoC が focus を奪う）。共有の机で前の app を長く奪わないよう 1 周だけ。終わったら元の前の app に戻す
    let active = open_textedit(&short, false).and_then(|_| run(rows, frames, "source_active", "a", 1));
    if !original_front.is_empty() && original_front != "TextEdit" {
        let _ = Command::new("/usr/bin/open").args(["-a", &original_front]).status();
    }
    // 片付け: 2 つの文書を閉じる（保存済みなので確認は出ない。未保存なら閉じずに残す）
    for w in [&ws, &wl] {
        let _ = helper_json(helper, &["input", "--window", &w.id.to_string(), "--pid", &pid.to_string(), r#"{"op":"key","key":"w","mods":["cmd"]}"#]);
    }
    inactive?;
    active
}

/// REAPER の今の値（edit cursor・time selection・arrange の見えている範囲）を ReaScript で読む。set が Some ならその値に戻してから読む
fn reaper_state(set: Option<[f64; 5]>) -> Result<[f64; 5], String> {
    let pid = audio::surface::pid_of(audio::surface::REAPER.process).ok_or("REAPER が起動していません")?;
    let bin = std::env::var_os("KANGAE_REAPER_BIN")
        .map(PathBuf::from)
        .or_else(|| audio::surface::exe_of(pid))
        .unwrap_or_else(|| PathBuf::from("/Applications/REAPER.app/Contents/MacOS/REAPER"));
    let restore = set
        .map(|s| format!("reaper.SetEditCurPos({}, false, false)\nreaper.GetSet_LoopTimeRange2(0, true, false, {}, {}, false)\nreaper.GetSet_ArrangeView2(0, true, 0, 0, {}, {})\n", s[0], s[1], s[2], s[3], s[4]))
        .unwrap_or_default();
    let text = audio::reaper::send(
        &bin,
        |done, cancel| {
            format!(
                "local function exists(p) local f = io.open(p, \"r\") if f then f:close() return true end return false end\n\
                 if exists([==[{cancel}]==]) then return end\n{restore}\
                 local cur = reaper.GetCursorPosition()\n\
                 local ts, te = reaper.GetSet_LoopTimeRange2(0, false, false, 0, 0, false)\n\
                 local as, ae = reaper.GetSet_ArrangeView2(0, false, 0, 0, 0, 0)\n\
                 local f = io.open([==[{done}]==], \"w\")\n\
                 f:write(string.format(\"%.6f\\t%.6f\\t%.6f\\t%.6f\\t%.6f\", cur, ts, te, as, ae))\nf:close()\n"
            )
        },
        audio::reaper::DEFAULT_TIMEOUT,
    )?
    .ok_or("REAPER が 90 秒以内に script を回しませんでした")?;
    let v: Vec<f64> = text.split('\t').filter_map(|s| s.trim().parse().ok()).collect();
    v.try_into().map_err(|_| format!("REAPER の読み戻しを読めません: {text:?}"))
}

/// REAPER（起動中のまま・前に出さない）: key（Home で先頭へ）・click（arrange を click → edit cursor）・横 scroll（見えている範囲）・
/// drag（time selection か cursor）。測る前の値を取り、終わったら戻す。track を足す・再生する操作は打たない
fn measure_reaper(helper: &Path, rows: &mut Vec<Value>, rounds: usize, frames: &mut Value) -> Result<(), String> {
    let pid = pid_of("REAPER").ok_or("REAPER が起動していません")?;
    let w = wait_window(helper, pid, "REAPER v", Duration::from_secs(10))?;
    let original = reaper_state(None).map_err(|e| format!("未測: REAPER の読み戻しが答えません（load {}）: {e}", load_avg()))?;
    let started = now_ms();
    let mut m = Measure { helper: helper.to_path_buf(), stream: Stream::start(helper, w.id, 960, 15)?, rows: Vec::new(), app: "REAPER", pid, title: w.title.clone() };
    std::thread::sleep(Duration::from_millis(2500));
    let o = original;
    let wait = || std::thread::sleep(Duration::from_millis(900));
    for round in 1..=rounds {
        m.trial("key", round, |m| {
            let s = reaper_state(Some([5.0, o[1], o[2], o[3], o[4]]))?;
            let (visible, noisy) = m.send(json!({"op": "key", "key": "home", "mods": []}))?;
            wait();
            let a = reaper_state(None)?;
            Ok(Outcome { visible, noisy, verified: Some(s[0] > 4.9 && a[0] < 0.001), detail: format!("cursor {:.3} → {:.3}", s[0], a[0]) })
        });
        m.trial("click", round, |m| {
            let s = reaper_state(Some([0.0, o[1], o[2], o[3], o[4]]))?;
            let (visible, noisy) = m.send(json!({"op": "click", "x": 0.55 + 0.05 * round as f64, "y": 0.6, "count": 1}))?;
            wait();
            let a = reaper_state(None)?;
            Ok(Outcome { visible, noisy, verified: Some((a[0] - s[0]).abs() > 0.001), detail: format!("cursor {:.3} → {:.3}", s[0], a[0]) })
        });
        m.trial("scroll", round, |m| {
            let s = reaper_state(None)?;
            let (visible, noisy) = m.send(json!({"op": "scroll", "x": 0.6, "y": 0.6, "dx": -400, "dy": 0}))?;
            wait();
            let a = reaper_state(None)?;
            Ok(Outcome { visible, noisy, verified: Some((a[3] - s[3]).abs() > 0.0001), detail: format!("arrange の左端 {:.3} → {:.3}", s[3], a[3]) })
        });
        m.trial("drag", round, |m| {
            let s = reaper_state(Some([0.0, 0.0, 0.0, o[3], o[4]]))?;
            let (visible, noisy) = m.send(json!({"op": "drag", "x": 0.5, "y": 0.9, "x2": 0.7, "y2": 0.9, "steps": 10}))?;
            wait();
            let a = reaper_state(None)?;
            let changed = (a[1] - s[1]).abs() > 0.001 || (a[2] - s[2]).abs() > 0.001 || (a[0] - s[0]).abs() > 0.001;
            Ok(Outcome { visible, noisy, verified: Some(changed), detail: format!("time selection {:.3}–{:.3} → {:.3}–{:.3}・cursor {:.3} → {:.3}", s[1], s[2], a[1], a[2], s[0], a[0]) })
        });
    }
    frames["reaper"] = frame_stats(&m.stream, started);
    rows.append(&mut m.rows);
    drop(m);
    reaper_state(Some(original)).map(|_| ()).map_err(|e| format!("REAPER を元の値に戻せませんでした: {e}"))
}

fn run_measure(target: &str, rounds: usize) -> Result<Value, String> {
    let helper = helper_path()?;
    let status = helper_json(&helper, &["status"]).map_err(|e| e.to_string())?;
    if status["screen_capture"] != true || status["post_event"] != true {
        return Err(format!("許可が足りません: {status}"));
    }
    let started = now_ms();
    let load_before = load_avg();
    let mut rows = Vec::new();
    let mut frames = json!({});
    let result = match target {
        "textedit" => {
            let dir = std::env::temp_dir().join(format!("kangae-mirror-{}", std::process::id()));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let r = measure_textedit(&helper, &mut rows, rounds, &dir, &mut frames);
            let _ = std::fs::remove_dir_all(&dir);
            r
        }
        "reaper" => measure_reaper(&helper, &mut rows, rounds, &mut frames),
        other => return Err(format!("measure の対象は textedit か reaper です（{other}）")),
    };
    Ok(json!({
        "target": target,
        "rounds": rounds,
        "via": "pid（CGEvent.postToPid・cursor を動かさない）",
        "load_before": load_before,
        "load_after": load_avg(),
        "seconds": ((now_ms() - started) / 100.0).round() / 10.0,
        "error": result.err(),
        "frames": frames,
        "summary": summarize(&rows),
        "rows": rows,
    }))
}

/// 日本語の face（Hiragino）を先頭に。egui 同梱の font には CJK が無く、無いと日本語が □ になる（実機の窓で確認）。
/// 本体は src/ui/theme.rs の fonts が同じ事をする（この bin は ui を取り込まないので、ここは最小の 1 face だけ）
fn japanese_fonts(ctx: &egui::Context) {
    let Some(data) = ["/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", "/System/Library/Fonts/Hiragino Sans GB.ttc"].iter().find_map(|p| std::fs::read(p).ok()) else { return };
    let mut f = egui::FontDefinitions::default();
    f.font_data.insert("KangaeJP".into(), egui::FontData::from_owned(data).into());
    f.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "KangaeJP".into());
    f.families.entry(egui::FontFamily::Monospace).or_default().push("KangaeJP".into());
    ctx.set_fonts(f);
}

// ============================== main ==============================

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None => {
            let options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default().with_title("Kangae Mirror").with_inner_size([1200.0, 800.0]),
                ..Default::default()
            };
            let app = Box::new(|cc: &eframe::CreationContext<'_>| -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>> {
                japanese_fonts(&cc.egui_ctx);
                Ok(Box::new(MirrorApp::new()))
            });
            match eframe::run_native("Kangae Mirror", options, app) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("窓を開けません: {e}");
                    1
                }
            }
        }
        Some("list") => match helper_path().and_then(|h| helper_json(&h, &["list", "--all"]).map_err(|e| e.to_string())) {
            Ok(v) => {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                0
            }
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        Some("measure") => {
            let target = args.get(1).cloned().unwrap_or_default();
            let rounds = args.iter().position(|a| a == "--rounds").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(3);
            match run_measure(&target, rounds) {
                Ok(v) => {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        Some(_) => {
            eprintln!("使い方: kangae-mirror [list | measure textedit|reaper [--rounds N]]");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reached_only_with_a_frame_after_the_input_and_view_only_after_two_silences() {
        let metas = [(4100.0, 4120.0)];
        let after = |t: f64| metas.iter().map(|(captured, _)| *captured).find(|c| *c > t);
        // 1.5 秒待っても frame が来ない → まだ view_only ではない（1 回目）
        let mut st = KindState { pending: Some((0.0, false)), ..KindState::default() };
        settle(&mut st, |_| None, 1000.0);
        assert!(st.pending.is_some(), "1.5 秒より前に決めない");
        settle(&mut st, |_| None, 1600.0);
        assert_eq!((st.reach.clone(), st.misses), (Reach::Unknown, 1));
        // 2 回目も来ない → view_only
        st.pending = Some((2000.0, false));
        settle(&mut st, |_| None, 3600.0);
        assert_eq!(st.reach, Reach::ViewOnly);
        // 送った後に撮った frame が来た → 届いた（100 ms）・数え直し
        st.pending = Some((4000.0, false));
        settle(&mut st, after, 4200.0);
        assert_eq!((st.reach.clone(), st.misses), (Reach::Reached { ms: 100.0 }, 0));
        // 送る前から画面が変わり続けている時は、frame が来ても証拠にしない
        st.pending = Some((4000.0, true));
        settle(&mut st, after, 4200.0);
        assert_eq!(st.reach, Reach::Noisy);
        // 送る前に撮った frame は数えない
        st.pending = Some((5000.0, false));
        settle(&mut st, after, 5100.0);
        assert!(st.pending.is_some());
    }
}
