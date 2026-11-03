//! 重ね（Native Direct Docking・v0.14 §5.1〜§5.3・PORTAL-*）: 元の窓を Page の枠に重ねて追従させる方式だけが使う物。
//! - 契約と状態: `AppSurfaceSpec`・`Lifecycle::next`（§5.2・図 §18 の表）・`portal_state`（Lifecycle → 枠の 9 状態）・重ねる窓を選ぶ `pick_main_window`
//! - Docking: `Dock` が PortalSlot の画面座標へ元の窓を合わせ（`Tracker` が送る分を合体・`Metrics` が実測）、bridge の event で追従・終了・人の操作を分ける
//! - 証拠: REAPER の .rpp の item を読み、2 つの写しの差を出す（PORTAL-14。見た目でなく保存 file で確かめる）・タイル型の窓管理 app の検出・Kangae の窓の前後
//! owner 23・24 通目（10-23「Mirror」）で常に重ねる方式は撤回。G91（owner 10-27「触る時だけ本物を枠に」「hideできないか」）で Mirror の面が `Dock` を使う:
//! 普段は本物の窓を枠と同じ位置・大きさで Shibubu の窓の下にしまい（`host_order` = `order_host_above`）、Mirror が送れない入力の時だけ前に出す
//! （`host_order` = `order_host_below`・`bring_front`）。呼び元は src/ui/mirror_view.rs（feature mirror）・src/project/native.rs（feature native_direct）・
//! src/bin/kangae-portal.rs・tests/portal.rs・generated.rs の cfg(native_direct)。消さない（R-del-native は native.rs の側だけ）。
//! 生きている型（Frame・WindowInfo・PortalSlot・PortalState・Host・ControlLease・Capability）と bridge の client は surface/mod.rs。
use super::*;

/// set_frame の読み戻しと枠の差がこれ以下なら合っている（points）。
pub const TOLERANCE_PT: f64 = 2.0;
/// 枠の動きがこれ未満なら送らない（points）。
const MOVE_EPSILON_PT: f64 = 0.5;
/// 追従中の set_frame の応答を待つ上限。過ぎたら degraded。
pub const REPLY_LIMIT: Duration = Duration::from_secs(3);
/// set_frame の応答の actual が枠と違った時、窓が落ち着くのを待ってから読み直すまで。
/// 実機（W16-3 pane N・2026-10-23）: 前に居る TextEdit でも set_frame の直後の actual が途中の値だった（460 を送って 440・510 を送って 462）。
/// すぐ同じ枠を送り直すと、動いている途中の窓に重なって落ち着いた先がまたずれた（1078×423 を送って 1064×424）
pub const SETTLE: Duration = Duration::from_millis(500);

const SAMPLE_CAP: usize = 10_000;

/// 入力を誰がどう届けるか（§5.1 control_mode）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    /// 元の app が人の入力を直接受ける（Native Direct・External）
    HumanDirect,
    /// Kangae が人の入力を転送する（Mirror）
    HumanForwarded,
    AgentAction,
    Hybrid,
}

/// Kangae が何を観測するか（§5.1 observation_mode）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationMode {
    None,
    Metadata,
    /// 窓の位置・大きさ・題名だけ（画面の中身は読まない）
    Accessibility,
    ScreenOptIn,
    Plugin,
    Api,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AppIdentity {
    pub bundle_id: String,
    pub name: String,
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub path: Option<String>,
}

/// Surface の契約（§5.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSurfaceSpec {
    pub surface_id: String,
    pub app_identity: AppIdentity,
    pub surface_backend: SurfaceBackend,
    pub control_mode: ControlMode,
    pub observation_mode: ObservationMode,
    pub sync_mode: SyncMode,
    pub capabilities: Vec<Capability>,
    pub permission_set: Vec<Permission>,
    pub host_slot: Option<PortalSlot>,
    pub source_window: Option<WindowInfo>,
    pub focus_owner: FocusOwner,
    pub fallback_chain: Vec<SurfaceBackend>,
    /// 直近に確かめた時刻と証拠
    pub last_verified: Option<(f64, String)>,
    pub user_layout_locked: bool,
}

/// Native Direct の fallback の順（§5.1 fallback_chain）。
pub fn native_direct_chain() -> Vec<SurfaceBackend> {
    vec![SurfaceBackend::NativeDirect, SurfaceBackend::ExternalHandoff]
}

/// chain の中で current の次（無ければ None）。
pub fn next_fallback(chain: &[SurfaceBackend], current: SurfaceBackend) -> Option<SurfaceBackend> {
    chain.iter().position(|b| *b == current).and_then(|i| chain.get(i + 1)).copied()
}

impl AppSurfaceSpec {
    /// Native Direct の初期値: どの層も Ready ではない（PORTAL-13）。Agent の GUI 操作は無い（CuaDriver に依らない）。
    pub fn native_direct(surface_id: &str, app: AppIdentity) -> AppSurfaceSpec {
        let unverified = |r: &str| Readiness::Unverified { reason: r.to_string() };
        let unsupported = |r: &str| Readiness::Unsupported { reason: r.to_string() };
        AppSurfaceSpec {
            surface_id: surface_id.to_string(),
            app_identity: app,
            surface_backend: SurfaceBackend::NativeDirect,
            control_mode: ControlMode::HumanDirect,
            observation_mode: ObservationMode::Accessibility,
            sync_mode: SyncMode::Unsupported,
            capabilities: vec![
                Capability { layer: Layer::Visual, actor: Actor::Human, readiness: unverified("Not overlaid yet") },
                Capability { layer: Layer::Input, actor: Actor::Human, readiness: unverified("The original app receives input directly. No human input observed yet") },
                Capability { layer: Layer::Input, actor: Actor::Agent, readiness: unsupported("This proof of concept has no agent GUI control") },
                Capability { layer: Layer::Semantic, actor: Actor::Human, readiness: unsupported("State read-back isn't connected") },
                Capability { layer: Layer::Semantic, actor: Actor::Agent, readiness: unsupported("State read-back isn't connected") },
            ],
            permission_set: vec![Permission {
                kind: PermissionKind::Accessibility,
                purpose: "To fit the window's position and size to the frame and follow its movement (screen contents are not read)".into(),
                state: PermissionState::NotRequested,
            }],
            host_slot: None,
            source_window: None,
            focus_owner: FocusOwner::Human,
            fallback_chain: native_direct_chain(),
            last_verified: None,
            user_layout_locked: false,
        }
    }

    pub fn readiness(&self, layer: Layer, actor: Actor) -> Option<&Readiness> {
        self.capabilities.iter().find(|c| c.layer == layer && c.actor == actor).map(|c| &c.readiness)
    }

    pub fn set_readiness(&mut self, layer: Layer, actor: Actor, readiness: Readiness) {
        if let Readiness::Ready { evidence, at_ms } = &readiness {
            self.last_verified = Some((*at_ms, evidence.clone()));
        }
        match self.capabilities.iter_mut().find(|c| c.layer == layer && c.actor == actor) {
            Some(c) => c.readiness = readiness,
            None => self.capabilities.push(Capability { layer, actor, readiness }),
        }
        // 同期の確度は表示と意味を分けて決める（§2.2）。意味が Ready なら定期確認、表示だけなら ライブ画面
        self.sync_mode = match (self.readiness(Layer::Semantic, Actor::Human).is_some_and(Readiness::is_ready), self.readiness(Layer::Visual, Actor::Human).is_some_and(Readiness::is_ready)) {
            (true, _) => SyncMode::SemanticPolled,
            (false, true) => SyncMode::VisualLive,
            (false, false) => SyncMode::Unsupported,
        };
    }

    /// 人が読む 1 行: 「表示: 確認済み · 操作: 未確認 · 状態: 非対応」
    pub fn status_line(&self) -> String {
        [Layer::Visual, Layer::Input, Layer::Semantic]
            .iter()
            .map(|l| format!("{}: {}", l.label(), self.readiness(*l, Actor::Human).map(Readiness::label).unwrap_or("Not supported")))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

// ---- Lifecycle（§5.2）----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Unresolved,
    Discovered,
    PermissionRequired,
    Attaching,
    Active,
    Hidden,
    Resizing,
    SourceUnavailable,
    Degraded,
    SwitchingBackend,
    Detached,
    Closed,
}

/// Lifecycle を動かす出来事（図 §18 の矢印の名前）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifeEvent {
    Found,
    NotFound,
    AttachRequested { trusted: bool },
    PermissionGranted,
    Cancel,
    Attached,
    AttachFailed,
    HostMoved,
    FrameOk,
    FrameOff,
    HostHidden,
    HostShown,
    HumanMoved,
    Redock,
    SourceGone,
    SourceBack,
    SourceAmbiguous,
    Handoff,
    HandoffDone,
    Close,
}

impl Lifecycle {
    /// 図 §18 の表。表に無い組は Err（状態を変えない）。Close は closed 以外のどこからでも。
    pub fn next(self, ev: LifeEvent) -> Result<Lifecycle, String> {
        use LifeEvent as E;
        use Lifecycle as L;
        let to = match (self, ev) {
            (L::Closed, _) => None,
            (_, E::Close) => Some(L::Closed),
            (L::Unresolved, E::Found) => Some(L::Discovered),
            (L::Unresolved, E::NotFound) => Some(L::Unresolved),
            (L::Discovered, E::AttachRequested { trusted: false }) => Some(L::PermissionRequired),
            (L::Discovered, E::AttachRequested { trusted: true }) => Some(L::Attaching),
            (L::PermissionRequired, E::PermissionGranted) => Some(L::Discovered),
            (L::PermissionRequired, E::Cancel) => Some(L::Closed),
            (L::Attaching, E::Attached) => Some(L::Active),
            (L::Attaching, E::AttachFailed) => Some(L::Degraded),
            (L::Active | L::Resizing, E::HostMoved) => Some(L::Resizing),
            (L::Resizing | L::Active, E::FrameOk) => Some(L::Active),
            (L::Resizing | L::Active, E::FrameOff) => Some(L::Degraded),
            (L::Active | L::Resizing, E::HostHidden) => Some(L::Hidden),
            (L::Hidden, E::HostShown) => Some(L::Active),
            (L::Active | L::Resizing, E::HumanMoved) => Some(L::Degraded),
            (L::Degraded | L::Detached, E::Redock) => Some(L::Attaching),
            (L::Active | L::Hidden | L::Resizing | L::Degraded | L::Attaching, E::SourceGone) => Some(L::SourceUnavailable),
            (L::SourceUnavailable, E::SourceBack) => Some(L::Attaching),
            (L::SourceUnavailable, E::SourceAmbiguous) => Some(L::SourceUnavailable),
            (L::Active | L::Hidden | L::Resizing | L::Degraded | L::SourceUnavailable, E::Handoff) => Some(L::SwitchingBackend),
            (L::SwitchingBackend, E::HandoffDone) => Some(L::Detached),
            _ => None,
        };
        to.ok_or_else(|| format!("That can't happen while it's {} (nothing changed)", self.label().to_lowercase()))
    }

    /// 元の窓を動かしてよい状態（hidden・degraded・detached・source_unavailable では触らない）
    pub fn touches_window(self) -> bool {
        matches!(self, Lifecycle::Attaching | Lifecycle::Active | Lifecycle::Resizing)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lifecycle::Unresolved => "unresolved",
            Lifecycle::Discovered => "discovered",
            Lifecycle::PermissionRequired => "permission_required",
            Lifecycle::Attaching => "attaching",
            Lifecycle::Active => "active",
            Lifecycle::Hidden => "hidden",
            Lifecycle::Resizing => "resizing",
            Lifecycle::SourceUnavailable => "source_unavailable",
            Lifecycle::Degraded => "degraded",
            Lifecycle::SwitchingBackend => "switching_backend",
            Lifecycle::Detached => "detached",
            Lifecycle::Closed => "closed",
        }
    }

    /// 画面に出す名前（§7.2）
    pub fn label(self) -> &'static str {
        match self {
            Lifecycle::Unresolved => "Not detected",
            Lifecycle::Discovered => "Detected",
            Lifecycle::PermissionRequired => "Needs permission",
            Lifecycle::Attaching => "Attaching",
            Lifecycle::Active => "Overlaid",
            Lifecycle::Hidden => "Shibubu is hidden",
            Lifecycle::Resizing => "Following",
            Lifecycle::SourceUnavailable => "Original app not found",
            Lifecycle::Degraded => "Not overlapping",
            Lifecycle::SwitchingBackend => "Switching",
            Lifecycle::Detached => "Open outside",
            Lifecycle::Closed => "Closed",
        }
    }
}

/// Lifecycle（12 状態）→ 枠の 9 状態。表は docs/diagrams.md §20 の対応（Active は backend で NativeDirect / Mirror に分け、
/// Mirror で `Readiness(Input, Human)` が Unsupported なら ViewOnly・`sync_mode` が Stale の Active は SyncDelayed・Detached は SyncDelayed）。
/// 例: `let shown = surface::portal_state(&dock.life, &dock.spec); if let Some(s) = shown { portal_frame::show(ui, id, &PortalFrame { state: s, .. }, |_| {}) }`
/// 保証する: Closed は None（枠を出さない）・Hidden / Resizing は直前の Active と同じ値（同じ spec なら同じ答え）・接続済みを示す NativeDirect / Mirror は Lifecycle が Active / Hidden / Resizing の時だけ。
/// 保証しない: 元の app が本当に応えている事（それは `project::readback` の receipt と試験の側の oracle）。
/// W2c の計画の署名 `portal_state(&Lifecycle)` からの変更: Lifecycle だけでは Active を NativeDirect / Mirror / ViewOnly に分けられないので spec を足し、Closed のために Option で返す。
/// Wave: W2c
pub fn portal_state(life: &Lifecycle, spec: &AppSurfaceSpec) -> Option<PortalState> {
    use Lifecycle as L;
    Some(match life {
        L::Closed => return None,
        L::Unresolved => PortalState::Undiscovered,
        L::Discovered | L::Attaching | L::SwitchingBackend => PortalState::Attaching,
        L::PermissionRequired => PortalState::PermissionRequired,
        L::SourceUnavailable => PortalState::SourceUnavailable,
        L::Degraded => PortalState::IntegrationFailed,
        L::Detached => PortalState::SyncDelayed,
        L::Active | L::Hidden | L::Resizing if spec.sync_mode == SyncMode::Stale => PortalState::SyncDelayed,
        L::Active | L::Hidden | L::Resizing => match spec.surface_backend {
            SurfaceBackend::InteractiveMirror => match spec.readiness(Layer::Input, Actor::Human) {
                Some(Readiness::Unsupported { .. }) => PortalState::ViewOnly,
                _ => PortalState::Mirror,
            },
            // 元の窓を外で開いている（External）は §20 の Detached と同じ: 同期遅延 + mode External
            SurfaceBackend::ExternalHandoff => PortalState::SyncDelayed,
            // 人の入力を元の物（窓・WebView・Plugin・生成した画面）が直接受ける
            SurfaceBackend::NativeDirect
            | SurfaceBackend::NativePortalExperimental
            | SurfaceBackend::WebView
            | SurfaceBackend::CooperativePlugin
            | SurfaceBackend::GeneratedUi => PortalState::NativeDirect,
        },
    })
}

/// 重ねる窓を 1 つ選ぶ: 普通の窓で最小化されていない・外から動かせる物のうち main。無ければ 1 つだけの物。決められなければ Err（人に前に出してもらう）。
/// 動かせない窓（REAPER の起動時の「About REAPER」の評価版の dialog は main だが位置を変えられない・実測 2026-10-18）は main でも選ばない。
pub fn pick_main_window(list: &[WindowInfo]) -> Result<WindowInfo, String> {
    let usable: Vec<&WindowInfo> = list.iter().filter(|w| w.standard() && !w.minimized).collect();
    let movable: Vec<&WindowInfo> = usable.iter().copied().filter(|w| w.movable).collect();
    let w = match (movable.iter().find(|w| w.main), movable.as_slice(), usable.as_slice()) {
        (Some(w), _, _) => *w,
        (None, [one], _) => *one,
        (None, [], [fixed, ..]) => return Err(format!("“{}” can't be moved or resized from outside. Use Open in the app instead", fixed.title_text())),
        (None, [], []) if list.iter().any(|w| w.minimized) => return Err("The window is minimised. Restore it from the Dock, then choose again".into()),
        (None, [], []) => return Err("No window to cover. Open a window of the app, then choose again".into()),
        (None, many, _) => return Err(format!("{} windows are open and Shibubu can't tell which to cover. Bring the one you want to the front, then choose again", many.len())),
    };
    if w.fullscreen {
        return Err(format!("“{}” is full screen. Leave full screen, then choose again", w.title_text()));
    }
    if w.frame.is_none() {
        return Err(format!("Can't read the position and size of “{}”", w.title_text()));
    }
    Ok(w.clone())
}

// ---- 追従（Tracker）と計測（Metrics）----

#[derive(Debug, Clone, Copy, PartialEq)]
struct InFlight {
    id: u64,
    target: Frame,
    changed_at: Instant,
}

/// set_frame を送る分を決める: 枠が動いた時だけ・応答待ちの間は最新の target だけ覚える（古い target は捨てる）。
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    /// 最後に読み戻しを受け取った target
    pub last_target: Option<Frame>,
    in_flight: Option<InFlight>,
    queued: Option<(Frame, Instant)>,
}

/// set_frame の応答 1 つの計測。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sample {
    pub error_px: f64,
    /// 枠が変わったのを見た時 → 読み戻しを受け取った時
    pub lag_ms: f64,
}

impl Tracker {
    /// 枠の画面座標が今 target。送るなら Some((target, 枠が変わった時刻))。応答待ちなら覚えて None
    pub fn want(&mut self, target: Frame, now: Instant) -> Option<(Frame, Instant)> {
        let latest = self.queued.map(|q| q.0).or(self.in_flight.map(|f| f.target)).or(self.last_target);
        if latest.is_some_and(|l| l.error_px(&target) < MOVE_EPSILON_PT) {
            return None;
        }
        if self.in_flight.is_some() {
            let since = self.queued.map(|q| q.1).unwrap_or(now);
            self.queued = Some((target, since));
            return None;
        }
        Some((target, now))
    }
    pub fn sent(&mut self, id: u64, target: Frame, changed_at: Instant) {
        self.in_flight = Some(InFlight { id, target, changed_at });
    }
    pub fn waiting(&self) -> Option<u64> {
        self.in_flight.map(|f| f.id)
    }
    /// 応答待ちのまま limit を過ぎたか
    pub fn overdue(&self, now: Instant, sent_at: Option<Instant>, limit: Duration) -> bool {
        self.in_flight.is_some() && sent_at.is_some_and(|s| now.duration_since(s) > limit)
    }
    /// 応答を受け取った。id が送った物なら (target, 計測)。actual が None（error）なら計測なし
    pub fn replied(&mut self, id: u64, actual: Option<Frame>, now: Instant) -> Option<(Frame, Option<Sample>)> {
        let f = self.in_flight.filter(|f| f.id == id)?;
        self.in_flight = None;
        self.last_target = Some(f.target);
        let sample = actual.map(|a| Sample { error_px: a.error_px(&f.target), lag_ms: now.duration_since(f.changed_at).as_secs_f64() * 1000.0 });
        Some((f.target, sample))
    }
    /// 応答待ちの間に覚えた最新の target
    pub fn take_queued(&mut self) -> Option<(Frame, Instant)> {
        self.queued.take()
    }
    /// 観測した frame が自分の送った target（応答待ち・覚えた物・最後の物）の echo か
    pub fn is_echo(&self, observed: &Frame) -> bool {
        [self.in_flight.map(|f| f.target), self.queued.map(|q| q.0), self.last_target].iter().flatten().any(|t| t.error_px(observed) <= TOLERANCE_PT)
    }
    pub fn reset(&mut self) {
        *self = Tracker::default();
    }
}

/// v0.14 §12.3 の指標のうち Docking で測れる物。値は決め打ちしない（全部実測の記録）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Metrics {
    pub samples: Vec<Sample>,
    /// 人の操作中に Kangae の手で frontmost が変わった回数
    pub focus_steal_events: u32,
    pub z_order_checks: u32,
    /// docked の窓が Kangae の窓より後ろに居た回数
    pub z_order_breaks: u32,
    pub human_moves: u32,
    pub timeouts: u32,
    /// Kangae 自身の窓を docked の窓の真下に置いた回数 / 失敗した回数（keep_front）
    pub order_calls: u32,
    pub order_failures: u32,
    /// 崩れた場面・測れなかった事（時刻つき）
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct Summary {
    pub n: usize,
    pub error_max: f64,
    pub error_p50: f64,
    pub error_p95: f64,
    pub lag_p50: f64,
    pub lag_p95: f64,
    pub lag_max: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

impl Metrics {
    pub fn record(&mut self, s: Sample) {
        if self.samples.len() >= SAMPLE_CAP {
            self.samples.remove(0);
        }
        self.samples.push(s);
    }
    pub fn note(&mut self, text: impl Into<String>) {
        if self.notes.len() >= 200 {
            self.notes.remove(0);
        }
        self.notes.push(text.into());
    }
    pub fn summary(&self) -> Summary {
        let mut e: Vec<f64> = self.samples.iter().map(|s| s.error_px).collect();
        let mut l: Vec<f64> = self.samples.iter().map(|s| s.lag_ms).collect();
        e.sort_by(f64::total_cmp);
        l.sort_by(f64::total_cmp);
        Summary {
            n: e.len(),
            error_max: e.last().copied().unwrap_or(0.0),
            error_p50: percentile(&e, 0.5),
            error_p95: percentile(&e, 0.95),
            lag_p50: percentile(&l, 0.5),
            lag_p95: percentile(&l, 0.95),
            lag_max: l.last().copied().unwrap_or(0.0),
        }
    }
    /// 画面の下の 1 行
    pub fn line(&self) -> String {
        let s = self.summary();
        if s.n == 0 {
            return format!("0 measurements · focus stolen {} · order broken {}/{}", self.focus_steal_events, self.z_order_breaks, self.z_order_checks);
        }
        format!(
            "Off by up to {:.1} pt · lag {:.0} ms median / {:.0} ms at 95% ({} measurements) · focus stolen {} · order broken {}/{}",
            s.error_max, s.lag_p50, s.lag_p95, s.n, self.focus_steal_events, self.z_order_breaks, self.z_order_checks
        )
    }
}

// ---- Dock（Native Direct Docking の 1 surface・図 §18 の追従）----

/// CG の前後の一覧（`cg_windows`・前ほど order が小さい）で、docked の窓（CG の窓番号 `cg`）と Kangae の一番手前の窓（`host_pid`・layer 0）の順
fn z_orders(v: &Value, cg: i64, host_pid: u32) -> (Option<u64>, Option<u64>) {
    let list = v["windows"].as_array().map(Vec::as_slice).unwrap_or_default();
    let order_of = |pred: &dyn Fn(&Value) -> bool| list.iter().filter(|w| pred(w)).filter_map(|w| w["order"].as_u64()).min();
    (order_of(&|w| w["id"].as_i64() == Some(cg)), order_of(&|w| w["pid"].as_u64() == Some(host_pid as u64) && w["layer"].as_i64() == Some(0)))
}

/// 送った要求が何だったか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    SetFrame,
    /// by_human: 人の操作（重ねた直後・枠の中の click）で元の app を前に出した。focus 奪取に数えない
    Raise { by_human: bool },
    ZCheck,
    /// G91 の tuck の keep_front: Shibubu の窓を動かす前に前後を読む（`Dock::tuck`）
    Tuck,
    Restore,
    Activate,
    ReconnectApps,
    ReconnectWindows,
    /// 読み戻しが枠と違った set_frame の後、落ち着いた窓の frame を読み直す（相手の枠は `settle_target`）
    Settle,
}

pub struct Dock {
    pub spec: AppSurfaceSpec,
    pub life: Lifecycle,
    pub lease: ControlLease,
    pub tracker: Tracker,
    pub metrics: Metrics,
    pub window: Option<WindowInfo>,
    /// attach した時の元の frame（外で開く・閉じる で戻す）
    pub original: Option<Frame>,
    /// 状態の 1 行（人の言葉・次の一手つき）
    pub reason: String,
    pub host_pid: u32,
    saved_title: String,
    pending: HashMap<u64, Pending>,
    sent_at: Option<Instant>,
    last_reconnect: Option<Instant>,
    /// 自分が activate を頼んだ時刻。次に来る元の app の app_activated 1 回はこれの結果として数えない
    /// （時間の窓では足りない: load 880 で 23 秒遅れて届き、人の操作と数えて 操作: 確認済み を立てた・実測 2026-10-18）
    own_activation: Option<Instant>,
    /// Kangae の窓の枠が透明（下の窓が透けて見える）。前後の判定で「真下で間に何も無い」を見えている扱いにする
    pub host_has_hole: bool,
    /// 起動中のタイル型の窓管理 app（AeroSpace 等）。窓を並べ直して取り返すので、外れた理由に名前を出す
    pub tiler: Option<String>,
    /// Kangae 自身の窓を CG の窓番号の真下に置く口（kangae-portal は `order_host_below`・test は偽物・無ければ AXRaise の道）
    pub host_order: Option<fn(u32) -> Result<(), String>>,
    /// 直前の前後の確認で、docked の窓が今の画面に在ったか（無ければ keep_front しない）
    docked_visible: bool,
    /// 重ねた直後に元の app を前に出して focus を渡す（kangae-portal: 人が「重ねる」を押した = true）。
    /// false（W16-1 の Page の面: Page を開いただけで前面を取らない）は代わりに `keep_front`（Kangae の窓を真下へ・activate しない）
    pub front_on_attach: bool,
    /// 読み戻しが枠と合わなかった送りを 1 回だけ送り直した（合った応答で戻す）。2 回続けて合わなければ degraded
    frame_retried: bool,
    /// 読み戻しが枠と違った時: この時刻に窓の frame を読み直す（`SETTLE` 後）と、読み直す相手の枠
    settle_at: Option<Instant>,
    settle_target: Option<Frame>,
    /// L5-STICK: 最後に前へ来た app の pid（bridge の `app_activated`・どの app でも）。読む側（Mirror の `tuck`）が `take` する = 来た事を 1 回だけ数える
    pub activated: Option<u32>,
    /// L5-STICK: Kangae（host_pid）が隠れている（⌘H・bridge の `app_hidden` / `app_unhidden`）。egui には合図が無い
    pub host_hidden: bool,
    /// G91: `host_order` は Shibubu を本物の真上に置く口（Mirror が本物を Shibubu の下にしまっている）。keep_front は前後を読み、本物が Shibubu より
    /// 手前の時だけ動かす（無条件だと Shibubu が本物の直上の段へ沈み、もう 1 つの Mirror の本物や間の窓が Shibubu の上に出る・L5-STICK）。AXRaise には落ちない
    pub tuck: bool,
    /// MIRROR-FIT-1101（owner 198 の 8「枠の方が app の表示の大きさに合うように」）: 枠が本物の大きさに合わせる Dock（Mirror の面）。既定 false =
    /// kangae-portal と W16-1 の重ねは今まで通り枠に合わせて本物の大きさを替える。true の間: set_frame の読み戻しは位置だけ比べる（大きさは本物の物を
    /// `window.frame` に入れて合ったと数える = 最小の大きさで断られた枠は本物の大きさに戻る）・本物が自分で大きさを替えた（位置は同じ）`resized` は
    /// 外れたと数えず `window.frame` を替える
    pub follow_size: bool,
}

impl Dock {
    pub fn new(app: AppIdentity, host_pid: u32) -> Dock {
        let name = app.name.clone();
        Dock {
            spec: AppSurfaceSpec::native_direct(&format!("portal-{}", app.bundle_id), app),
            life: Lifecycle::Unresolved,
            lease: ControlLease::human(now_ms()),
            tracker: Tracker::default(),
            metrics: Metrics::default(),
            window: None,
            original: None,
            reason: format!("Looking for {name}"),
            host_pid,
            saved_title: String::new(),
            pending: HashMap::new(),
            sent_at: None,
            last_reconnect: None,
            own_activation: None,
            host_has_hole: false,
            tiler: None,
            host_order: None,
            docked_visible: true,
            front_on_attach: true,
            frame_retried: false,
            settle_at: None,
            settle_target: None,
            activated: None,
            host_hidden: false,
            tuck: false,
            follow_size: false,
        }
    }

    pub fn app_pid(&self) -> Option<u32> {
        self.spec.app_identity.pid
    }

    fn go(&mut self, ev: LifeEvent) {
        match self.life.next(ev) {
            Ok(to) => self.life = to,
            Err(e) => self.metrics.note(e),
        }
    }

    fn degrade(&mut self, reason: String) {
        if self.life.touches_window() || self.life == Lifecycle::Hidden {
            let ev = if self.life == Lifecycle::Attaching { LifeEvent::AttachFailed } else { LifeEvent::FrameOff };
            match self.life.next(ev) {
                Ok(to) => self.life = to,
                Err(_) => self.life = Lifecycle::Degraded,
            }
        }
        self.metrics.note(format!("{:.0} Limited: {reason}", now_ms()));
        self.reason = reason;
        self.tracker.reset();
        self.sent_at = None;
        self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Unverified { reason: self.reason.clone() });
    }

    fn send(&mut self, b: &mut Bridge, cmd: &str, args: Value, kind: Pending) -> Option<u64> {
        match b.send(cmd, args) {
            Ok(id) => {
                self.pending.insert(id, kind);
                Some(id)
            }
            Err(e) => {
                self.degrade(e.to_string());
                None
            }
        }
    }

    /// 人が「重ねる」を押した（人の操作で 1 回）: 許可 → 窓を選ぶ → 観測 → 枠へ合わせ始める。
    pub fn attach(&mut self, b: &mut Bridge, host: Host) {
        let name = self.spec.app_identity.name.clone();
        let Some(pid) = self.app_pid() else {
            self.reason = format!("{name} isn't running. Start it, then choose it");
            return;
        };
        if host.fullscreen {
            // 全画面の Kangae は別の Space に居るので、元の窓を重ねられない（実測 2026-10-18: 重ねると窓の高さが合わない）
            self.reason = "Shibubu is full screen. The original window stays in the previous Space, so it can't be overlaid (exit full screen to realign)".into();
            return;
        }
        let trusted = b.call("permission", json!({}), b.limit()).ok().and_then(|v| v["trusted"].as_bool()).unwrap_or(false);
        match self.life {
            Lifecycle::Unresolved => {
                self.go(LifeEvent::Found);
                self.go(LifeEvent::AttachRequested { trusted });
            }
            Lifecycle::Discovered => self.go(LifeEvent::AttachRequested { trusted }),
            Lifecycle::PermissionRequired if trusted => {
                self.go(LifeEvent::PermissionGranted);
                self.go(LifeEvent::AttachRequested { trusted: true });
            }
            Lifecycle::Degraded | Lifecycle::Detached if trusted => self.go(LifeEvent::Redock),
            _ => {}
        }
        if !trusted {
            self.spec.permission_set[0].state = PermissionState::NotRequested;
            self.reason = if self.life == Lifecycle::PermissionRequired {
                "Accessibility is off".into()
            } else {
                "Accessibility access was turned off (turn it on in System Settings, then Realign)".into()
            };
            return;
        }
        if self.life != Lifecycle::Attaching {
            return self.metrics.note(format!("Attaching does nothing while {}", self.life.as_str()));
        }
        self.spec.permission_set[0].state = PermissionState::Granted;
        let list = match b.call("windows", json!({ "pid": pid }), b.limit()) {
            Ok(v) => parse_windows(&v),
            Err(e) => return self.degrade(format!("Couldn't read the {name} window: {e}")),
        };
        let w = match pick_main_window(&list) {
            Ok(w) => w,
            Err(e) => return self.degrade(e),
        };
        self.start(b, w, host);
    }

    /// W16-1: 呼ぶ側が選んだ窓（`project::native::pick_document_window`）に重ね始める。許可は呼ぶ側が確かめた後（trusted）。
    /// `attach` と違い `pick_main_window` で選び直さない（README.md の窓を別の窓にすり替えない・L4）。
    pub fn attach_to(&mut self, b: &mut Bridge, w: WindowInfo, host: Host) {
        if self.life == Lifecycle::Unresolved {
            self.go(LifeEvent::Found);
        }
        if self.life == Lifecycle::Discovered {
            self.go(LifeEvent::AttachRequested { trusted: true });
        }
        if self.life != Lifecycle::Attaching {
            return self.metrics.note(format!("Attaching to a window does nothing while {}", self.life.as_str()));
        }
        self.spec.permission_set[0].state = PermissionState::Granted;
        self.start(b, w, host);
    }

    /// 窓が決まった後: 元の frame を覚え、観測し、枠へ送る。
    fn start(&mut self, b: &mut Bridge, w: WindowInfo, host: Host) {
        // 元の frame は、その窓に初めて触った時の物を持ち続ける（合わせ直すたびに今の frame = 枠の位置で上書きしない）
        if self.original.is_none() || self.window.as_ref().map(|x| x.handle.as_str()) != Some(w.handle.as_str()) {
            self.original = w.frame;
        }
        self.saved_title = w.title.clone();
        self.spec.source_window = Some(w.clone());
        self.window = Some(w);
        self.lease = ControlLease::human(now_ms());
        self.tracker.reset();
        self.spec.surface_backend = SurfaceBackend::NativeDirect;
        match b.call("observe", json!({ "pid": self.app_pid() }), b.limit()) {
            Ok(v) => {
                let failed: Vec<String> = v["failed"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default();
                if !failed.is_empty() {
                    self.metrics.note(format!("Notification that can't be observed: {}", failed.join(", ")));
                }
            }
            Err(e) => self.metrics.note(format!("Couldn't start watching (no change detection): {e}")),
        }
        self.reason = format!("Fitting {}'s window to the frame", self.spec.app_identity.name);
        self.on_host(b, host, Instant::now());
    }

    /// 1 frame ごと: Kangae の窓の今を受け取り、要れば set_frame を送る。
    pub fn on_host(&mut self, b: &mut Bridge, host: Host, now: Instant) {
        if host.minimized {
            if matches!(self.life, Lifecycle::Active | Lifecycle::Resizing) {
                self.go(LifeEvent::HostHidden);
                self.reason = "Shibubu is minimized (the original window is unchanged)".into();
            }
            return;
        }
        if self.life == Lifecycle::Hidden {
            self.go(LifeEvent::HostShown);
            self.reason = format!("Overlaying {}'s window on the frame (a separate window, not a true child view)", self.spec.app_identity.name);
        }
        if host.fullscreen && matches!(self.life, Lifecycle::Active | Lifecycle::Resizing) {
            return self.degrade("Shibubu is full screen. The original window stays in the previous Space, so it can't be overlaid (exit full screen to realign)".into());
        }
        let limit = REPLY_LIMIT.max(b.limit());
        if self.tracker.overdue(now, self.sent_at, limit) {
            self.metrics.timeouts += 1;
            let name = self.spec.app_identity.name.clone();
            return self.degrade(format!("{name} didn't answer within {} s (check that no dialog is open and the Mac isn't busy)", limit.as_secs()));
        }
        let (Some(target), true) = (host.slot, self.life.touches_window()) else { return };
        if let Some((t, since)) = self.tracker.want(target, now) {
            self.send_frame(b, t, since, now);
        }
    }

    fn send_frame(&mut self, b: &mut Bridge, t: Frame, since: Instant, now: Instant) {
        let Some(handle) = self.window.as_ref().map(|w| w.handle.clone()) else { return };
        if self.life == Lifecycle::Active {
            self.go(LifeEvent::HostMoved);
        }
        // 新しい枠を送る: 前の枠の読み直しは要らない
        self.settle_at = None;
        if let Some(id) = self.send(b, "set_frame", json!({ "handle": handle, "x": t.x, "y": t.y, "w": t.w, "h": t.h }), Pending::SetFrame) {
            self.tracker.sent(id, t, since);
            self.sent_at = Some(now);
        }
    }

    /// bridge から来た 1 つ。
    pub fn on_msg(&mut self, b: &mut Bridge, msg: Msg, now: Instant) {
        match msg {
            Msg::Reply { id, result } => self.on_reply(b, id, result, now),
            Msg::Event(v) => self.on_event(b, &v, now),
            Msg::Broken(s) => self.metrics.note(format!("Unreadable line from the bridge: {s}")),
            Msg::Gone(why) => self.degrade(BridgeError::Gone(why).to_string()),
        }
    }

    /// 読み戻し `a` が送った枠 `t` からずれた大きさ（point）。MIRROR-FIT-1101: `follow_size` の Dock は位置だけ（大きさは本物の物 = 合ったと数えて
    /// `frame_ok` が `window.frame` に入れる = 最小の大きさで断られた枠は本物の大きさに戻る）・それ以外は今まで通り位置と大きさの最大
    fn miss(&self, a: &Frame, t: &Frame) -> f64 {
        if self.follow_size {
            (a.x - t.x).abs().max((a.y - t.y).abs())
        } else {
            a.error_px(t)
        }
    }

    /// set_frame の読み戻しが枠と合った（直後の actual か、落ち着いた後の読み直し）: Attached / FrameOk・覚えた次の枠を送る・前後を保つ。
    fn frame_ok(&mut self, b: &mut Bridge, actual: Option<Frame>, now: Instant) {
        let name = self.spec.app_identity.name.clone();
        self.frame_retried = false;
        if let Some(w) = self.window.as_mut() {
            w.frame = actual;
        }
        let first = self.life == Lifecycle::Attaching;
        self.go(if first { LifeEvent::Attached } else { LifeEvent::FrameOk });
        self.reason = format!("Overlaying {name}'s window on the frame (a separate window, not a true child view)");
        if let Some((next, since)) = self.tracker.take_queued() {
            if self.life.touches_window() {
                self.send_frame(b, next, since, now);
            }
        } else if first && self.front_on_attach {
            // 重ねた直後: 人は元の app を使うために選んだので、前に出して focus も渡す（奪取に数えない）
            self.bring_front(b);
        } else {
            // 枠が動いて合った直後: Kangae の窓が前に出ていても docked の窓を上に保つ
            self.keep_front(b);
        }
    }

    fn on_reply(&mut self, b: &mut Bridge, id: u64, result: Result<Value, BridgeError>, now: Instant) {
        let Some(kind) = self.pending.remove(&id) else { return };
        let name = self.spec.app_identity.name.clone();
        match (kind, result) {
            (Pending::SetFrame, Ok(v)) => {
                let actual = Frame::from_json(&v["actual"]);
                let Some((target, sample)) = self.tracker.replied(id, actual, now) else { return };
                self.sent_at = None;
                let Some(s) = sample else { return self.degrade(format!("Reading back {name}'s window returned nothing")) };
                self.metrics.record(s);
                // MIRROR-FIT-1101: 大きさを本物に任せる Dock（follow_size）は位置だけ比べる
                if actual.map_or(s.error_px, |a| self.miss(&a, &target)) > TOLERANCE_PT {
                    // L5-STICK ②: 待つ間に枠がまた動いた（Shibubu の窓を drag している間）→ 古い枠の読み直しは要らない。新しい枠をすぐ送る
                    // （待つと覚えた枠が SETTLE の 0.5 秒届かず、本物が遅れて付いて来る）
                    // B33（bfae19b2 の独立 review 黄）: Hidden（Shibubu が最小化・⌘H）の間は覚えた枠を送らない = 本物の窓に触らない（`frame_ok` と同じ確かめ。
                    // 覚えた枠は持ったまま = 戻った後の読み戻しか `on_host` が送る）
                    if self.life.touches_window() {
                        if let Some((next, since)) = self.tracker.take_queued() {
                            return self.send_frame(b, next, since, now);
                        }
                    }
                    // 直後の actual は動いている途中の事がある（SETTLE）: 落ち着くのを待って読み直し、ずれていた時だけ送り直す（`Pending::Settle`）
                    self.settle_at = Some(now + SETTLE);
                    self.settle_target = Some(target);
                    return;
                }
                self.frame_ok(b, actual, now);
            }
            (Pending::Settle, Ok(v)) => {
                // 読み直しの間に枠が動いて新しい set_frame を送った: 古い枠の読みは捨てる
                let Some(target) = self.settle_target.take() else { return };
                if self.tracker.waiting().is_some() || self.tracker.last_target != Some(target) {
                    return;
                }
                let actual = Frame::from_json(&v["frame"]);
                if actual.is_some_and(|a| self.miss(&a, &target) <= TOLERANCE_PT) {
                    return self.frame_ok(b, actual, now);
                }
                // 実機（W16-1 3 回目）: 背景の TextEdit は最初の set_frame を途中の大きさ（936×443・1058×447 など）で止める事がある。
                // 同じ枠をもう 1 回送ると 6/6 で合った。送り直すのは 1 回だけ（最小の大きさより小さい枠など、合わない枠で回り続けない）
                if !self.frame_retried {
                    self.frame_retried = true;
                    return self.send_frame(b, target, now, now);
                }
                let a = actual.map(|a| a.text()).unwrap_or_default();
                self.degrade(format!("{name}'s window won't fit the frame (frame {}, actual {a}). Enlarge the frame, or use Open in the app", target.text()));
            }
            (Pending::Settle, Err(e)) => {
                self.settle_target = None;
                self.degrade(e.to_string());
            }
            (Pending::SetFrame, Err(e)) => {
                self.tracker.replied(id, None, now);
                self.sent_at = None;
                if e.code() == "ax_invalid_ui_element" {
                    return self.source_gone(format!("{name}'s window was closed"));
                }
                self.degrade(e.to_string());
            }
            (Pending::Raise { by_human }, Ok(v)) => {
                let (before, after) = (v["front_before"].as_u64().unwrap_or(0) as u32, v["front_after"].as_u64().unwrap_or(0) as u32);
                if by_human && Some(before) == self.app_pid() {
                    // 元から前に居た: activate しても app_activated は来ないので、待っている 1 回を消す
                    self.own_activation = None;
                }
                if after != before && Some(after) == self.app_pid() && !by_human {
                    // 人が Kangae を使っている間に、Kangae の手で元の app が前に来た
                    self.metrics.focus_steal_events += 1;
                    self.metrics.note(format!("{:.0} Focus was taken: process {before} → {after}", now_ms()));
                } else if after != before && after != self.host_pid && Some(after) != self.app_pid() {
                    self.metrics.note(format!("{:.0} Another app (process {after}) came to the front, not caused by Shibubu", now_ms()));
                }
                self.z_check(b);
            }
            (Pending::Raise { .. }, Err(e)) => self.metrics.note(format!("Couldn't move it back: {e}")),
            (Pending::ZCheck, Ok(v)) => self.on_z(&v),
            (Pending::ZCheck | Pending::Tuck, Err(e)) => self.metrics.note(format!("Couldn't read the window order: {e}")),
            (Pending::Tuck, Ok(v)) => {
                // 本物が Shibubu より手前（Shibubu が一覧に無い = 読めない時も）の時だけ Shibubu を本物の真上へ。既に手前なら動かさない（動かすと下の本物の直上へ沈む）
                // ponytail: 2 つの本物が同時に Shibubu より手前で、古い一覧で 2 つとも動かすと後の方の直上に沈み得る。tuck の本物が前に出るのは本物の app が前に来た時（tuck = false）だけなので起きにくい
                let cg = self.window.as_ref().and_then(|w| w.cg_id);
                let (docked, host) = cg.map_or((None, None), |cg| z_orders(&v, cg, self.host_pid));
                // 読みを頼んだ後に直接触る間へ替わった（本物の app が前に来た lift・go_direct = tuck が false・host_order は order_below）: 古い読みで Shibubu を動かさない
                if let (true, Some(order), Some(cg), Some(d)) = (self.tuck, self.host_order, cg, docked) {
                    if host.map_or(true, |h| d < h) {
                        match order(cg as u32) {
                            Ok(()) => {
                                self.metrics.order_calls += 1;
                                return self.z_check(b);
                            }
                            Err(e) => {
                                self.metrics.order_failures += 1;
                                self.metrics.note(format!("{:.0} Couldn't place Shibubu directly above the original window ({e})", now_ms()));
                            }
                        }
                    }
                }
                self.on_z(&v)
            }
            (Pending::Restore, r) => {
                if let Err(e) = r {
                    self.metrics.note(format!("Couldn't restore the original position: {e}"));
                }
                let pid = self.app_pid();
                if let Some(h) = self.window.as_ref().map(|w| w.handle.clone()) {
                    self.own_activation = Some(Instant::now());
                    self.send(b, "raise", json!({ "handle": h, "activate": true, "pid": pid }), Pending::Activate);
                } else {
                    self.finish_handoff();
                }
            }
            (Pending::Activate, r) => {
                if let Err(e) = r {
                    self.metrics.note(format!("Couldn't bring {name} to the front: {e}"));
                }
                self.finish_handoff();
            }
            (Pending::ReconnectApps, Ok(v)) => {
                let pid = v["apps"].as_array().and_then(|a| a.first()).and_then(|a| a["pid"].as_u64()).map(|p| p as u32);
                if let Some(pid) = pid {
                    self.spec.app_identity.pid = Some(pid);
                    self.send(b, "windows", json!({ "pid": pid }), Pending::ReconnectWindows);
                }
            }
            (Pending::ReconnectWindows, Ok(v)) => {
                let list = parse_windows(&v);
                let bundle = self.spec.app_identity.bundle_id.clone();
                match pick_reconnect(&bundle, &self.saved_title, &bundle, &list) {
                    Reconnect::One(w) => {
                        self.go(LifeEvent::SourceBack);
                        self.metrics.note(format!("{:.0} Reconnected to “{}” ({})", now_ms(), w.title, w.handle));
                        let host = Host { slot: self.tracker.last_target, ..Host::default() };
                        self.start(b, w, host);
                    }
                    Reconnect::Ask(why) | Reconnect::None(why) => {
                        self.go(LifeEvent::SourceAmbiguous);
                        self.reason = why;
                    }
                }
            }
            (Pending::ReconnectApps | Pending::ReconnectWindows, Err(e)) => self.metrics.note(format!("Couldn't confirm the reconnection: {e}")),
        }
    }

    fn finish_handoff(&mut self) {
        self.go(LifeEvent::HandoffDone);
        self.spec.surface_backend = SurfaceBackend::ExternalHandoff;
        self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Unverified { reason: "Open in the original window".into() });
        self.reason = format!("{} is open in its original window (press Return to Shibubu to cover it again)", self.spec.app_identity.name);
    }

    /// CG の前後（前から後ろの順）で、docked の窓が Kangae の窓より手前かを見る。
    fn on_z(&mut self, v: &Value) {
        let list = v["windows"].as_array().cloned().unwrap_or_default();
        // attach の時に対応が付かなかった（tiling の途中等で frame がずれていた）: 今の frame が一致する同じ pid の窓が 1 つだけなら、それ
        if self.window.as_ref().is_some_and(|w| w.cg_id.is_none()) {
            let (pid, frame) = (self.app_pid(), self.window.as_ref().and_then(|w| w.frame));
            let hits: Vec<i64> = list
                .iter()
                .filter(|w| w["pid"].as_u64().map(|p| p as u32) == pid && w["layer"].as_i64() == Some(0))
                .filter(|w| Frame::from_json(&w["frame"]).zip(frame).is_some_and(|(a, b)| a.error_px(&b) <= 1.0))
                .filter_map(|w| w["id"].as_i64())
                .collect();
            if let (Some(w), [one]) = (self.window.as_mut(), hits.as_slice()) {
                w.cg_id = Some(*one);
            }
        }
        let Some(cg) = self.window.as_ref().and_then(|w| w.cg_id) else {
            self.metrics.note("Can't check the window order because the windows can't be matched up");
            return;
        };
        // 応答が来る前に重なりが外れた（人が動かした・degraded）なら、前後が合っていても表示を確認済みにしない
        if !matches!(self.life, Lifecycle::Active | Lifecycle::Resizing) {
            return self.metrics.note(format!("The window order check is skipped while {}", self.life.as_str()));
        }
        let (docked, host) = z_orders(v, cg, self.host_pid);
        self.docked_visible = docked.is_some();
        self.metrics.z_order_checks += 1;
        // Kangae の枠が透明なら、真下に居て間に枠と重なる他の窓が無い時も見えている
        let frame = self.window.as_ref().and_then(|w| w.frame);
        let app_pid = self.app_pid().map(|p| p as u64);
        let host_pid = self.host_pid as u64;
        let between = |lo: u64, hi: u64| {
            list.iter().any(|w| {
                let o = w["order"].as_u64().unwrap_or(0);
                let pid = w["pid"].as_u64();
                o > lo && o < hi && w["layer"].as_i64() == Some(0) && pid != Some(host_pid) && pid != app_pid
                    && Frame::from_json(&w["frame"]).zip(frame).is_some_and(|(a, f)| a.x < f.x + f.w && f.x < a.x + a.w && a.y < f.y + f.h && f.y < a.y + a.h)
            })
        };
        match (docked, host) {
            // G91 の tuck: 本物が Shibubu の後ろに居るのが正しい（見えるのは Mirror の映像）
            (Some(d), Some(h)) if self.tuck && h < d => {
                let at = now_ms();
                self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Ready { evidence: "The read-back frame is within tolerance and the original window is behind Shibubu's window".into(), at_ms: at });
            }
            (Some(d), Some(h)) if self.tuck => {
                self.metrics.z_order_breaks += 1;
                self.metrics.note(format!("{:.0} The original window is in front of Shibubu: docked window at {d}, Shibubu at {h}", now_ms()));
                self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Unverified { reason: "The original window is on top of Shibubu".into() });
            }
            (Some(d), Some(h)) if d < h => {
                let at = now_ms();
                self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Ready { evidence: "The read-back frame is within tolerance of the frame and in front of Shibubu's window".into(), at_ms: at });
            }
            (Some(d), Some(h)) if self.host_has_hole && !between(h, d) => {
                let at = now_ms();
                self.spec.set_readiness(
                    Layer::Visual,
                    Actor::Human,
                    Readiness::Ready { evidence: "The read-back frame is within tolerance and directly below Shibubu's transparent frame (no overlapping window in between)".into(), at_ms: at },
                );
            }
            (None, _) => {
                self.metrics.z_order_breaks += 1;
                self.metrics.note(format!("{:.0} The docked window isn't on this screen (another Space or minimised)", now_ms()));
                self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Unverified { reason: "The original window isn't on this screen".into() });
            }
            (Some(_), None) => self.metrics.note("The Shibubu window is missing from the window order"),
            (Some(d), Some(h)) => {
                self.metrics.z_order_breaks += 1;
                self.metrics.note(format!("{:.0} Window order is wrong: docked window at {d}, Shibubu at {h}", now_ms()));
                self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Unverified { reason: "Hidden behind the Shibubu window".into() });
            }
        }
    }

    /// G91 改 2: 外れた（`Degraded`）窓を、同じ窓のまま枠（`host.slot`）へ 1 回送り直す（`Redock` → Attaching・窓を選び直さず observe もし直さない）。
    /// 送り直しは 1 回で set_frame 1 つ（`frame_retried` を立てて送る・読み戻しが合わなければ送り直さずにまた `Degraded`）。
    /// 返す値 = 送り直しを始めた。窓が無い（閉じた・app が終わった）・外れていない時は何もしない（false）。
    /// 例: `if d.life == Lifecycle::Degraded { d.redock(b, Host { slot, ..Host::default() }, Instant::now()); }`
    pub fn redock(&mut self, b: &mut Bridge, host: Host, now: Instant) -> bool {
        if self.life != Lifecycle::Degraded || self.window.is_none() {
            return false;
        }
        self.go(LifeEvent::Redock);
        self.reason = format!("Fitting {}'s window to the frame", self.spec.app_identity.name);
        (self.frame_retried, self.settle_at, self.settle_target) = (true, None, None);
        self.tracker.reset();
        self.on_host(b, host, now);
        true
    }

    /// 人の操作で元の窓を前に出す（重ねた直後・Kangae の枠の中の click）: AXRaise + activate。focus は元の app へ
    pub fn bring_front(&mut self, b: &mut Bridge) {
        let (Some(h), true) = (self.window.as_ref().map(|w| w.handle.clone()), matches!(self.life, Lifecycle::Active | Lifecycle::Resizing)) else { return };
        self.own_activation = Some(Instant::now());
        self.send(b, "raise", json!({ "handle": h, "activate": true }), Pending::Raise { by_human: true });
    }

    /// Kangae が前に来た・click された・枠が動いた直後: docked の窓を Kangae の窓より上に保つ（v0.14 P3 で採った公開 API の道）。
    /// Kangae 自身の窓を docked の窓の真下に置く（activate しない = focus は Kangae のまま）。docked の窓が消えた・今の画面に無い時は呼ばない。
    /// 口が無い（host_order が None）か失敗した時は、前の道（AXRaise だけ・透明な枠の真下で見せる）に戻る。
    /// G91 の tuck（`tuck`・向きが逆 = 本物を Shibubu の下に保つ）: 前後を読み、本物が Shibubu より手前の時だけ Shibubu を本物の真上へ（AXRaise に落ちない）。
    pub fn keep_front(&mut self, b: &mut Bridge) {
        if !matches!(self.life, Lifecycle::Active | Lifecycle::Resizing) || !self.docked_visible {
            return;
        }
        let Some(w) = self.window.as_ref() else { return };
        if self.tuck {
            // 前後を読んでから決める（on_reply の Pending::Tuck）
            self.send(b, "cg_windows", json!({ "on_screen_only": true }), Pending::Tuck);
            return;
        }
        let (handle, cg) = (w.handle.clone(), w.cg_id);
        match (self.host_order, cg) {
            (Some(order), Some(cg)) => match order(cg as u32) {
                Ok(()) => {
                    self.metrics.order_calls += 1;
                    return self.z_check(b);
                }
                Err(e) => {
                    self.metrics.order_failures += 1;
                    self.metrics.note(format!("{:.0} Couldn't place Shibubu directly below ({e}). Raising the window instead", now_ms()));
                }
            },
            (Some(_), None) => self.metrics.note("The docked window has no number, so Shibubu can't go directly below. Raising the window instead"),
            (None, _) => {}
        }
        self.send(b, "raise", json!({ "handle": handle, "activate": false }), Pending::Raise { by_human: false });
    }

    pub fn z_check(&mut self, b: &mut Bridge) {
        self.send(b, "cg_windows", json!({ "on_screen_only": true }), Pending::ZCheck);
    }

    fn source_gone(&mut self, reason: String) {
        if self.life.next(LifeEvent::SourceGone).is_ok() {
            self.go(LifeEvent::SourceGone);
        } else if self.life != Lifecycle::SourceUnavailable && self.life != Lifecycle::Closed && self.life != Lifecycle::Detached {
            self.metrics.note(format!("The original window disappeared while {}", self.life.as_str()));
        }
        self.reason = format!("{reason}. Restart and Shibubu reconnects to the window with the same title");
        self.tracker.reset();
        self.sent_at = None;
        self.window = None;
        self.spec.source_window = None;
        self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Unverified { reason: "No original window".into() });
    }

    fn on_event(&mut self, b: &mut Bridge, v: &Value, now: Instant) {
        let pid = v["pid"].as_u64().map(|p| p as u32);
        let handle = v["handle"].as_str();
        let ours = self.window.as_ref().is_some_and(|w| Some(w.handle.as_str()) == handle);
        let name = self.spec.app_identity.name.clone();
        match (v["event"].as_str(), pid) {
            (Some("app_activated"), Some(p)) => self.activated = Some(p),
            (Some("app_hidden"), Some(p)) if p == self.host_pid => self.host_hidden = true,
            (Some("app_unhidden"), Some(p)) if p == self.host_pid => self.host_hidden = false,
            _ => {}
        }
        match v["event"].as_str().unwrap_or("") {
            "moved" | "resized" if ours => {
                let Some(f) = Frame::from_json(&v["frame"]) else { return };
                // MIRROR-FIT-1101: 大きさを本物に任せる Dock は、本物の今の frame を覚える。位置が同じ（本物が自分で大きさだけ替えた）なら外れたと数えない = 枠が追う
                if self.follow_size {
                    let held = self.window.as_ref().and_then(|w| w.frame).is_some_and(|o| (o.x - f.x).abs().max((o.y - f.y).abs()) <= TOLERANCE_PT);
                    if let Some(w) = self.window.as_mut() {
                        w.frame = Some(f);
                    }
                    if held {
                        return;
                    }
                }
                if self.tracker.is_echo(&f) || !matches!(self.life, Lifecycle::Active | Lifecycle::Resizing) {
                    return;
                }
                self.metrics.human_moves += 1;
                self.go(LifeEvent::HumanMoved);
                self.reason = match &self.tiler {
                    Some(t) => format!("{name}'s window was moved from outside, so following stopped ({t} may have rearranged it: set {name} and Shibubu to floating in {t}, then Realign)"),
                    None => format!("{name}'s window was moved from outside, so following stopped (press Realign to restore)"),
                };
                self.tracker.reset();
                self.spec.set_readiness(Layer::Visual, Actor::Human, Readiness::Unverified { reason: "The window left the frame".into() });
            }
            // G91: Shibubu が自分で最小化した（Mirror の黄・先に Hidden にしてから送る）窓は外れたと数えない
            "minimized" if ours && self.life != Lifecycle::Hidden => self.degrade(format!("{name}'s window was minimized (restore it, then Realign)")),
            "destroyed" if ours => self.source_gone(format!("{name}'s window was closed")),
            "app_terminated" if pid.is_some() && pid == self.app_pid() => {
                self.source_gone(format!("{name} quit"));
                self.spec.app_identity.pid = None;
            }
            "app_launched" if v["bundle_id"].as_str() == Some(self.spec.app_identity.bundle_id.as_str()) => {
                self.spec.app_identity.pid = pid;
                self.last_reconnect = None;
            }
            "app_activated" if pid == Some(self.host_pid) => {
                // Kangae が前に来た: docked の窓を上に保つ（Kangae の窓を真下へ・口が無ければ AXRaise だけ。どちらも activate しない）
                self.keep_front(b);
            }
            "app_activated" if pid.is_some() && pid == self.app_pid() => {
                // 自分の activate の結果（60 秒以内の最初の 1 回）は人の操作に数えない
                let ours = self.own_activation.take().is_some_and(|t| t.elapsed() < Duration::from_secs(60));
                if !ours && matches!(self.life, Lifecycle::Active | Lifecycle::Resizing) {
                    self.lease.human_input(now_ms());
                    self.spec.set_readiness(
                        Layer::Input,
                        Actor::Human,
                        Readiness::Ready { evidence: format!("{name} came to the front while overlaid (a human click)"), at_ms: now_ms() },
                    );
                }
            }
            "space_changed" if matches!(self.life, Lifecycle::Active | Lifecycle::Resizing) => self.z_check(b),
            _ => {}
        }
        let _ = now;
    }

    /// source_unavailable の間、1 秒ごとに app を探して繋ぎ直しを試す（UI の 1 frame から呼ぶ）。
    pub fn tick(&mut self, b: &mut Bridge, now: Instant) {
        if self.settle_at.is_some_and(|t| now >= t) {
            self.settle_at = None;
            if let Some(h) = self.window.as_ref().map(|w| w.handle.clone()) {
                self.send(b, "frame", json!({ "handle": h }), Pending::Settle);
            }
        }
        if self.life != Lifecycle::SourceUnavailable || self.pending.values().any(|p| matches!(p, Pending::ReconnectApps | Pending::ReconnectWindows)) {
            return;
        }
        if self.last_reconnect.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        self.last_reconnect = Some(now);
        let bundle = self.spec.app_identity.bundle_id.clone();
        self.send(b, "apps", json!({ "bundle_id": bundle }), Pending::ReconnectApps);
    }

    /// 外で開く（§3.2-6・fallback の最後）: drag の途中なら Err。元の frame に戻して元の app を前に出す。
    pub fn handoff(&mut self, b: &mut Bridge) -> Result<(), String> {
        let buttons = b.call("mouse", json!({}), b.limit()).ok().and_then(|v| v["buttons"].as_u64()).unwrap_or(0);
        self.lease.may_switch_backend(buttons)?;
        self.life.next(LifeEvent::Handoff)?;
        self.go(LifeEvent::Handoff);
        self.tracker.reset();
        self.sent_at = None;
        match (self.window.as_ref().map(|w| w.handle.clone()), self.original) {
            (Some(h), Some(o)) => {
                self.send(b, "set_frame", json!({ "handle": h, "x": o.x, "y": o.y, "w": o.w, "h": o.h }), Pending::Restore);
            }
            _ => {
                // 窓が無い（app が終了している）: 起動して前に出す
                let bundle = self.spec.app_identity.bundle_id.clone();
                self.own_activation = Some(Instant::now());
                self.send(b, "open_app", json!({ "bundle_id": bundle, "activate": true }), Pending::Activate);
            }
        }
        Ok(())
    }

    /// 切断 / Kangae を閉じる: 元の frame に戻して閉じる（人の操作で 1 回・待つ）。
    pub fn close(&mut self, b: &mut Bridge) {
        if let (Some(h), Some(o), true) = (self.window.as_ref().map(|w| w.handle.clone()), self.original, self.life != Lifecycle::Detached) {
            if let Err(e) = b.call("set_frame", json!({ "handle": h, "x": o.x, "y": o.y, "w": o.w, "h": o.h }), b.limit()) {
                self.metrics.note(format!("Couldn't restore the original position on close: {e}"));
            }
        }
        if let Some(pid) = self.app_pid() {
            let _ = b.send("unobserve", json!({ "pid": pid }));
        }
        self.go(LifeEvent::Close);
        self.reason = format!("Put {}'s window back in its original position", self.spec.app_identity.name);
    }
}

// ---- 証拠: REAPER の .rpp（PORTAL-14）----

/// .rpp の 1 item（保存 file から読む。REAPER の GUID は .rpp の IGUID と同じ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RppItem {
    pub guid: String,
    pub track: String,
    pub take: String,
    pub position: f64,
    pub length: f64,
}

/// `KEY "a b"` / `KEY 'a'` / `KEY word` の値（REAPER は " を含む名前を ' か ` で囲む）。
fn rpp_value(rest: &str) -> String {
    let rest = rest.trim();
    let mut chars = rest.chars();
    match chars.next() {
        Some(q @ ('"' | '\'' | '`')) => chars.take_while(|c| *c != q).collect(),
        Some(_) => rest.split_whitespace().next().unwrap_or("").to_string(),
        None => String::new(),
    }
}

/// .rpp の本文から item の一覧（ブロックの入れ子を数えて、TRACK の NAME と ITEM の POSITION / LENGTH / IGUID / NAME を拾う）。
pub fn rpp_items(text: &str) -> Vec<RppItem> {
    let mut stack: Vec<String> = Vec::new();
    let mut track = String::new();
    let mut items = Vec::new();
    let mut cur: Option<RppItem> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(tag) = line.strip_prefix('<') {
            let tag = tag.split_whitespace().next().unwrap_or("").to_string();
            if tag == "TRACK" {
                track.clear();
            }
            if tag == "ITEM" {
                cur = Some(RppItem { guid: String::new(), track: track.clone(), take: String::new(), position: 0.0, length: 0.0 });
            }
            stack.push(tag);
            continue;
        }
        if line == ">" {
            if stack.pop().as_deref() == Some("ITEM") {
                items.extend(cur.take());
            }
            continue;
        }
        let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match (stack.last().map(String::as_str), key) {
            (Some("TRACK"), "NAME") => track = rpp_value(rest),
            (Some("ITEM"), "POSITION") => cur.iter_mut().for_each(|c| c.position = rest.trim().parse().unwrap_or(c.position)),
            (Some("ITEM"), "LENGTH") => cur.iter_mut().for_each(|c| c.length = rest.trim().parse().unwrap_or(c.length)),
            (Some("ITEM"), "IGUID") => cur.iter_mut().for_each(|c| c.guid = rpp_value(rest)),
            (Some("ITEM"), "NAME") => cur.iter_mut().filter(|c| c.take.is_empty()).for_each(|c| c.take = rpp_value(rest)),
            _ => {}
        }
    }
    items
}

/// 2 つの写しの item の差（人の言葉の行）。位置・長さは 1ms 未満の差を同じとみなす。
pub fn diff_items(before: &[RppItem], after: &[RppItem]) -> Vec<String> {
    let mut out = Vec::new();
    let label = |i: &RppItem| format!("{} (track “{}” {})", if i.take.is_empty() { "item" } else { &i.take }, i.track, i.guid);
    for a in after {
        match before.iter().find(|b| b.guid == a.guid) {
            None => out.push(format!("Added: {} at {:.3} s, length {:.3} s", label(a), a.position, a.length)),
            Some(b) => {
                if (a.position - b.position).abs() >= 0.001 {
                    out.push(format!("Moved: {} from {:.3} to {:.3} s", label(a), b.position, a.position));
                }
                if (a.length - b.length).abs() >= 0.001 {
                    out.push(format!("Length changed: {} {:.3} → {:.3} s", label(a), b.length, a.length));
                }
                if a.track != b.track {
                    out.push(format!("Track changed: {} “{}” → “{}”", label(a), b.track, a.track));
                }
            }
        }
    }
    for b in before.iter().filter(|b| !after.iter().any(|a| a.guid == b.guid)) {
        out.push(format!("Removed: {}", label(b)));
    }
    out
}

/// 起動中の REAPER から読み戻した今の状態（kangae-portal の Lua が返す。何も変えない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReaperState {
    /// 今の project の .rpp（未保存なら ""）
    pub project: String,
    /// 未保存の変更がある（IsProjectDirty）
    pub dirty: bool,
    /// 最後の操作の名前（Undo_CanUndo2）
    pub last_undo: String,
    pub items: Vec<RppItem>,
}

/// Lua の応答: 1 行目 `proj \t file \t dirty \t 最後の操作`、続く行 `item \t GUID \t 位置 \t 長さ \t take 名 \t track 名` か `err \t 原文`。
pub fn parse_reaper_state(text: &str) -> Result<ReaperState, String> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let f: Vec<&str> = first.splitn(4, '\t').collect();
    match f[0] {
        "err" => return Err(format!("Failed inside REAPER: {}", f.get(1).unwrap_or(&""))),
        "proj" if f.len() >= 3 => {}
        _ => return Err(format!("Couldn't read REAPER's reply: {}", cut(first))),
    }
    let mut items = Vec::new();
    for l in lines {
        let g: Vec<&str> = l.splitn(6, '\t').collect();
        let bad = || format!("Can't read the REAPER item line: {}", cut(l));
        if g.len() != 6 || g[0] != "item" {
            return Err(bad());
        }
        items.push(RppItem {
            guid: g[1].to_string(),
            position: g[2].trim().parse().map_err(|_| bad())?,
            length: g[3].trim().parse().map_err(|_| bad())?,
            take: g[4].to_string(),
            track: g[5].to_string(),
        });
    }
    Ok(ReaperState { project: f[1].to_string(), dirty: f[2].trim() != "0", last_undo: f.get(3).unwrap_or(&"").to_string(), items })
}

/// 窓を自動で並べ直すタイル型の窓管理 app（bundle id → 名前）。重ねた窓を取り返すので、外れた理由に出す。
/// 載せるのは実物で bundle id を確かめた物だけ（AeroSpace 0.21.3: /Applications/AeroSpace.app の Info.plist・2026-10-18）。
pub const KNOWN_TILERS: &[(&str, &str)] = &[("bobko.aerospace", "AeroSpace")];

/// bridge の `apps`（regular_only: false）の result から、起動中のタイル型の窓管理 app の名前。
pub fn running_tiler(apps: &Value) -> Option<String> {
    apps["apps"].as_array()?.iter().find_map(|a| {
        let id = a["bundle_id"].as_str()?;
        KNOWN_TILERS.iter().find(|(b, _)| *b == id).map(|(_, n)| n.to_string())
    })
}

// ---- Kangae 自身の窓の前後（mod.rs の `appkit` を呼ぶ・重ねの方式だけが使う）----

/// Kangae 自身の窓（この process の key → main → 最初の窓）を、CG の窓番号 `target` の窓の真下に置く
/// （`-[NSWindow orderWindow:NSWindowBelow relativeTo:]`・公開 API・v0.14 P3 で 100/100・activate しないので focus は動かない）。
/// AppKit の main thread から呼ぶ（eframe の update の中）。AppKit が読み込まれていない process（test など）では Err。
#[cfg(target_os = "macos")]
pub fn order_host_below(target: u32) -> Result<(), String> {
    appkit(AppKitOp::OrderHostBelow(target)).map(|_| ())
}

#[cfg(not(target_os = "macos"))]
pub fn order_host_below(_target: u32) -> Result<(), String> {
    Err("Only macOS can reorder the Shibubu window".into())
}

/// W16-1: Kangae 自身の窓を CG の窓番号 `target` の窓の**真上**に置く（`-[NSWindow orderWindow:NSWindowAbove relativeTo:]`・公開 API・activate しない）。
/// 面の Page を離れた時に重ねた窓を Kangae の後ろへ隠す（窓は閉じない・動かさない = 戻った時に同じ窓 id）。前面の app・focus・他の app の窓との前後は変えない
/// （Kangae は元の窓の直上の 1 段に入るだけ）。AppKit の main thread から呼ぶ。AppKit の無い process では Err。
#[cfg(target_os = "macos")]
pub fn order_host_above(target: u32) -> Result<(), String> {
    appkit(AppKitOp::OrderHostAbove(target)).map(|_| ())
}

#[cfg(not(target_os = "macos"))]
pub fn order_host_above(_target: u32) -> Result<(), String> {
    Err("Only macOS can reorder the Shibubu window".into())
}

/// L5-STICK ①・SINGLE-INSTANCE: pid の app を前へ（`NSRunningApplication activateWithOptions:`・公開 API・窓には触らない）。AppKit の main thread から呼ぶ。
/// 保証しない: 前に出る事（macOS 14 の協調の activate では、前でない process の依頼を系が断る事がある・実機は未測）。AppKit の無い process では Err。
#[cfg(target_os = "macos")]
pub fn activate_pid(pid: u32) -> Result<(), String> {
    appkit(AppKitOp::Activate(pid)).map(|_| ())
}

#[cfg(not(target_os = "macos"))]
pub fn activate_pid(_pid: u32) -> Result<(), String> {
    Err("Only macOS can bring an app to the front".into())
}
