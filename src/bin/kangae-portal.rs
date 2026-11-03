//! Native Direct Docking の PoC（v0.14 §11 P1・§13・DESIGN §17-3）。元の app の本物の窓を Kangae の PortalSlot に重ね、
//! Kangae の窓の移動 / 大きさに付いていかせる。入力は元の app が直接受ける（Kangae は click を再実行しない）。
//! **真の子 View ではない**（別の窓を重ねるだけ）。非公開 API・CuaDriver は使わない。図は docs/diagrams.md §18。
//!   kangae-portal                          窓を開く → app を選ぶ → 許可 → 重ねる（外で開く で普通の窓に戻る）
//!   kangae-portal --app <bundle id>        起動したらその app を選ぶ
//!   kangae-portal --app <id> --measure OUT 重ねた後に Kangae の窓を program で動かして追従を測り、OUT（JSON）に書いて閉じる（合成）
//!   kangae-portal probe [bundle id]        bridge・app・窓の一覧を 1 回だけ出す（窓を動かさない）
//!   kangae-portal evidence save NAME [--rpp FILE]  REAPER の今の item（読むだけ）と .rpp の item を写す
//!   kangae-portal evidence diff A B        2 つの写しの差（PORTAL-03 / 04 / 14 の証拠）
//! bridge は `KANGAE_SURFACE_BRIDGE` → 実行 file の隣 → ../Helpers → ../bridge（`surface::find_bridge`）。

#[allow(dead_code)]
#[path = "../surface/mod.rs"]
mod surface;
// ui は agent（rail）と domain（renderer・connections）を crate root の sibling として引く（src/main.rs と同じ並び）
#[allow(dead_code, unused_imports)]
#[path = "../domain/mod.rs"]
mod domain;
#[allow(dead_code, unused_imports)]
#[path = "../agent/mod.rs"]
mod agent;
// shell の Intent が model::AiAccount を運ぶ。model は時刻だけ google を、google は crypto を引く
#[allow(dead_code)]
#[path = "../crypto.rs"]
mod crypto;
#[allow(dead_code)]
#[path = "../google/mod.rs"]
mod google;
#[allow(dead_code)]
#[path = "../model.rs"]
mod model;
#[allow(dead_code, unused_imports)]
#[path = "../ui/mod.rs"]
mod ui;
#[allow(dead_code)]
#[path = "../audio/mod.rs"]
mod audio;

use eframe::egui::{self, pos2, vec2, Align, Layout, Rect, Sense, Stroke, StrokeKind};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use surface::{AppIdentity, Bridge, BridgeError, Dock, Frame, Host, LifeEvent, Lifecycle, Msg, PortalSlot};
use ui::skeleton::{self, Skeleton};
use ui::theme::{self, Type, Variant};

const REAPER_ID: &str = "com.cockos.reaper";
const BAR_H: f32 = 44.0;
const FOOT_H: f32 = 24.0;

const USAGE: &str = "使い方:
  kangae-portal [--app BUNDLE_ID] [--measure OUT.json]
  kangae-portal probe [BUNDLE_ID]
  kangae-portal evidence save NAME [--rpp FILE]
  kangae-portal evidence diff A B
  kangae-portal web URL [--app ID] [--space ID] [--allow HOST,HOST] [--quit-after S]
  kangae-portal web --youtube VIDEO_ID [--no-referer] …   YouTube の埋め込み（Referer: https://app.kangae.desktop・--no-referer は負の対照）
  kangae-portal web --store-test --app ID …               app ごとの data store を自前の頁（127.0.0.1）で確かめる
    web は Kangae の窓の中の本物の WKWebView（feature webview の build だけ）。無い build・開けない site は本人の browser で（handoff）
環境変数: KANGAE_SURFACE_BRIDGE（bridge の場所）・KANGAE_BRIDGE_AX_TIMEOUT（AX の応答を待つ秒・既定 3）・KANGAE_REAPER_BIN・
          KANGAE_PORTAL_LOG（状態の移り変わりを書く file）・KANGAE_OPEN_BIN（handoff の open の差し替え）";

/// 研究の Portal の帯の backend の名前（呼ぶのはここだけ・UI-2 で surface::SurfaceBackend::label から移した = 製品の画面には出ない）
fn backend_label(b: surface::SurfaceBackend) -> &'static str {
    use surface::SurfaceBackend as S;
    match b {
        S::WebView => "WebView",
        S::NativeDirect => "Native Direct（重ねて表示）",
        S::NativePortalExperimental => "Native Portal（研究）",
        S::InteractiveMirror => "Mirror",
        S::CooperativePlugin => "Plugin",
        S::ExternalHandoff => "External（元の窓）",
        S::GeneratedUi => "Kangae の画面",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("probe") => std::process::exit(probe(args.get(1).map(String::as_str).unwrap_or(REAPER_ID))),
        Some("evidence") => std::process::exit(evidence(&args[1..])),
        Some("web") => std::process::exit(web(&args[1..])),
        Some("-h" | "--help") => println!("{USAGE}"),
        _ => {
            let mut want_app = None;
            let mut measure = None;
            let mut it = args.into_iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--app" => want_app = it.next(),
                    "--measure" => measure = it.next().map(PathBuf::from),
                    other => {
                        eprintln!("不明な引数: {other}\n{USAGE}");
                        std::process::exit(2);
                    }
                }
            }
            if measure.is_some() && want_app.is_none() {
                eprintln!("--measure には --app が要ります\n{USAGE}");
                std::process::exit(2);
            }
            let options = eframe::NativeOptions {
                // 透明な窓: 重ねている間は枠を塗らず、真下の元の窓を透かして見せる（AXRaise は activate しない限り前の app の窓より上に出ないため・図 §18）
                viewport: egui::ViewportBuilder::default()
                    .with_title("Kangae Portal")
                    .with_inner_size([1280.0, 800.0])
                    .with_min_inner_size([720.0, 480.0])
                    .with_transparent(true),
                ..Default::default()
            };
            let result = eframe::run_native(
                "Kangae Portal",
                options,
                Box::new(move |cc| {
                    theme::install(&cc.egui_ctx, egui::ThemePreference::System);
                    Ok(Box::new(Portal::new(want_app, measure.map(Measure::new))))
                }),
            );
            if let Err(e) = result {
                eprintln!("窓を開けません: {e}");
                std::process::exit(1);
            }
        }
    }
}

// ---- 窓 ----

#[derive(Debug, Clone)]
struct AppRow {
    pid: Option<u32>,
    bundle_id: String,
    name: String,
    version: Option<String>,
    path: Option<String>,
}

impl AppRow {
    fn identity(&self) -> AppIdentity {
        AppIdentity { bundle_id: self.bundle_id.clone(), name: self.name.clone(), pid: self.pid, version: self.version.clone(), path: self.path.clone() }
    }
}

struct Portal {
    boot: Option<mpsc::Receiver<Result<Bridge, BridgeError>>>,
    bridge: Option<Bridge>,
    bridge_err: Option<String>,
    /// 読み込みを始めた ctx の時刻（skeleton の started_at）
    loading_since: f64,
    apps: Option<Vec<AppRow>>,
    apps_req: Option<u64>,
    find_req: Option<u64>,
    reaper: Option<AppRow>,
    open_req: Option<(u64, AppRow)>,
    perm_req: Option<u64>,
    perm_checked: f64,
    tiler_req: Option<u64>,
    tiler: Option<String>,
    /// app を開いた後、窓が出るまで attach を試し直す期限（ctx の時刻）
    retry_until: Option<f64>,
    retry_at: f64,
    dock: Option<Dock>,
    want_app: Option<String>,
    note: Option<String>,
    slot: Option<Rect>,
    my_pid: u32,
    closed: bool,
    measure: Option<Measure>,
    /// KANGAE_PORTAL_LOG（file）に状態の変化を 1 行ずつ書く（実機の確認で、画面を見ずに状態の移り変わりを残す）
    state_log: Option<PathBuf>,
    last_logged: String,
}

impl Portal {
    fn new(want_app: Option<String>, measure: Option<Measure>) -> Portal {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = surface::find_bridge().and_then(|p| Bridge::spawn(&p, surface::BRIDGE_READY_LIMIT));
            let _ = tx.send(r);
        });
        Portal {
            boot: Some(rx),
            bridge: None,
            bridge_err: None,
            loading_since: 0.0,
            apps: None,
            apps_req: None,
            find_req: None,
            reaper: None,
            open_req: None,
            perm_req: None,
            perm_checked: 0.0,
            tiler_req: None,
            tiler: None,
            retry_until: None,
            retry_at: 0.0,
            dock: None,
            want_app,
            note: None,
            slot: None,
            my_pid: std::process::id(),
            closed: false,
            measure,
            state_log: std::env::var_os("KANGAE_PORTAL_LOG").map(PathBuf::from),
            last_logged: String::new(),
        }
    }

    /// Kangae の窓の今（枠の画面座標 = viewport の中身の左上 + 枠の窓内の位置）
    fn host(&self, ctx: &egui::Context) -> Host {
        ctx.input(|i| {
            let v = i.viewport();
            let slot = match (v.inner_rect, self.slot) {
                (Some(inner), Some(s)) => {
                    let in_host = Frame { x: s.min.x as f64, y: s.min.y as f64, w: s.width() as f64, h: s.height() as f64 };
                    Some(PortalSlot { id: "main".into(), in_host }.on_screen(inner.min.x as f64, inner.min.y as f64))
                }
                _ => None,
            };
            Host { slot, minimized: v.minimized.unwrap_or(false), fullscreen: v.fullscreen.unwrap_or(false) }
        })
    }

    fn boot_poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.boot else { return };
        match rx.try_recv() {
            Ok(Ok(mut b)) => {
                self.boot = None;
                self.loading_since = ctx.input(|i| i.time);
                self.apps_req = b.send("apps", json!({ "regular_only": true })).ok();
                self.find_req = b.send("find_app", json!({ "bundle_id": REAPER_ID })).ok();
                self.tiler_req = b.send("apps", json!({ "regular_only": false })).ok();
                self.bridge = Some(b);
            }
            Ok(Err(e)) => {
                self.boot = None;
                self.bridge_err = Some(e.to_string());
                if let Some(m) = self.measure.as_mut() {
                    m.fail(&e.to_string());
                }
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.boot = None;
                self.bridge_err = Some("bridge を起こす thread が止まりました".into());
            }
        }
    }

    /// bridge から来た物を、自分の要求の応答と dock の分に分ける。
    fn route(&mut self, ctx: &egui::Context) {
        let Some(mut b) = self.bridge.take() else { return };
        for m in b.poll() {
            match m {
                Msg::Reply { id, result } if Some(id) == self.apps_req => {
                    self.apps_req = None;
                    match result {
                        Ok(v) => self.apps = Some(parse_apps(&v, self.my_pid)),
                        Err(e) => {
                            self.apps = Some(Vec::new());
                            self.note = Some(format!("起動中の app を読めません: {e}"));
                        }
                    }
                }
                Msg::Reply { id, result } if Some(id) == self.find_req => {
                    self.find_req = None;
                    if let Ok(v) = result.as_ref().map(|v| v.clone()) {
                        if v["installed"].as_bool() == Some(true) {
                            self.reaper = Some(AppRow {
                                pid: None,
                                bundle_id: REAPER_ID.into(),
                                name: "REAPER".into(),
                                version: v["version"].as_str().map(String::from),
                                path: v["path"].as_str().map(String::from),
                            });
                        }
                    }
                }
                Msg::Reply { id, result } if self.open_req.as_ref().is_some_and(|o| o.0 == id) => {
                    let Some((_, mut row)) = self.open_req.take() else { continue };
                    match result {
                        Ok(v) => {
                            row.pid = v["pid"].as_u64().map(|p| p as u32);
                            self.retry_until = Some(ctx.input(|i| i.time) + 60.0);
                            self.select(&mut b, row, ctx);
                        }
                        Err(e) => self.note = Some(format!("{} を開けません: {e}", row.name)),
                    }
                }
                Msg::Reply { id, result } if Some(id) == self.tiler_req => {
                    self.tiler_req = None;
                    self.tiler = result.ok().and_then(|v| surface::running_tiler(&v));
                    if let (Some(d), Some(t)) = (self.dock.as_mut(), &self.tiler) {
                        d.tiler = Some(t.clone());
                    }
                }
                Msg::Reply { id, result } if Some(id) == self.perm_req => {
                    self.perm_req = None;
                    let trusted = result.ok().and_then(|v| v["trusted"].as_bool()).unwrap_or(false);
                    if trusted {
                        let host = self.host(ctx);
                        if let Some(d) = self.dock.as_mut() {
                            d.attach(&mut b, host);
                        }
                    }
                }
                other => match self.dock.as_mut() {
                    Some(d) => d.on_msg(&mut b, other, Instant::now()),
                    None => {
                        if let Msg::Gone(why) = other {
                            self.bridge_err = Some(BridgeError::Gone(why).to_string());
                        }
                    }
                },
            }
        }
        self.bridge = Some(b);
        // --app: 一覧が来たら選ぶ
        if let (Some(want), Some(apps)) = (self.want_app.clone(), self.apps.clone()) {
            if self.dock.is_none() && self.open_req.is_none() && self.find_req.is_none() {
                self.want_app = None;
                let row = apps.into_iter().find(|a| a.bundle_id == want).or_else(|| self.reaper.clone().filter(|r| r.bundle_id == want));
                match (row, self.bridge.take()) {
                    (Some(row), Some(mut b)) => {
                        self.select(&mut b, row, ctx);
                        self.bridge = Some(b);
                    }
                    (None, b) => {
                        self.bridge = b;
                        let why = format!("{want} が見つかりません");
                        if let Some(m) = self.measure.as_mut() {
                            m.fail(&why);
                        }
                        self.note = Some(why);
                    }
                    (Some(_), None) => {}
                }
            }
        }
    }

    fn select(&mut self, b: &mut Bridge, row: AppRow, ctx: &egui::Context) {
        self.note = None;
        if row.pid.is_none() {
            match b.send("open_app", json!({ "bundle_id": row.bundle_id, "activate": false })) {
                Ok(id) => self.open_req = Some((id, row)),
                Err(e) => self.note = Some(format!("{} を開けません: {e}", row.name)),
            }
            return;
        }
        let mut dock = Dock::new(row.identity(), self.my_pid);
        dock.host_has_hole = true;
        // Kangae が前に来ても docked の窓を上に保つ口（公開 API・v0.14 P3・docs/V14_P3_RESEARCH.md）。
        // KANGAE_PORTAL_ORDER=off は計測の負の対照だけ（前の道 = AXRaise と透明な枠に戻す）
        if std::env::var("KANGAE_PORTAL_ORDER").map_or(true, |v| v != "off") {
            dock.host_order = Some(surface::order_host_below);
        }
        dock.tiler = self.tiler.clone();
        dock.attach(b, self.host(ctx));
        self.dock = Some(dock);
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) {
            if d.life != Lifecycle::Closed {
                d.close(b);
            }
        }
    }

    fn log_state(&mut self) {
        let (Some(path), Some(d)) = (&self.state_log, &self.dock) else { return };
        let line = format!("{}\t{}\t{}", d.life.as_str(), d.spec.status_line(), d.reason);
        if line == self.last_logged {
            return;
        }
        self.last_logged = line.clone();
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{:.0}\t{line}", surface::now_ms());
        }
    }

    /// 1 frame ごとの追従。重ねている間は 30Hz で frame を回す（窓の移動を見るため）。
    fn follow(&mut self, ctx: &egui::Context) {
        let host = self.host(ctx);
        let now_t = ctx.input(|i| i.time);
        let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) else { return };
        d.on_host(b, host, Instant::now());
        d.tick(b, Instant::now());
        // app を開いた直後は窓が出るまで 1 秒ごとに試し直す
        if let Some(until) = self.retry_until {
            if d.window.is_some() || now_t > until {
                self.retry_until = None;
            } else if d.life == Lifecycle::Degraded && now_t >= self.retry_at {
                self.retry_at = now_t + 1.0;
                d.attach(b, host);
            }
        }
        if d.life == Lifecycle::PermissionRequired && self.perm_req.is_none() && now_t - self.perm_checked > 1.0 {
            self.perm_checked = now_t;
            self.perm_req = b.send("permission", json!({})).ok();
        }
        if !matches!(d.life, Lifecycle::Closed | Lifecycle::Detached) {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    // ---- 描画 ----

    fn bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = theme::tokens(ui.ctx());
        ui.horizontal_centered(|ui| {
            match &self.dock {
                Some(d) => {
                    theme::label(ui, Type::BodyStrong, &d.spec.app_identity.name, t.text_primary);
                    theme::label(ui, Type::Helper, format!("{} · {}", d.life.label(), backend_label(d.spec.surface_backend)), t.text_secondary);
                }
                None => {
                    theme::label(ui, Type::BodyStrong, "Kangae Portal", t.text_primary);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.actions(ui, ctx));
        });
    }

    fn actions(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let host = self.host(ctx);
        let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) else { return };
        let mut drop_dock = false;
        match d.life {
            Lifecycle::Active | Lifecycle::Resizing | Lifecycle::Hidden | Lifecycle::Degraded | Lifecycle::SourceUnavailable => {
                if theme::button_compact(ui, "切断", Variant::Ghost).clicked() {
                    d.close(b);
                    drop_dock = true;
                }
                if theme::button_compact(ui, "外で開く", Variant::Ghost).clicked() {
                    if let Err(e) = d.handoff(b) {
                        d.reason = e;
                    }
                }
                if d.life == Lifecycle::Degraded && theme::button_compact(ui, "合わせ直す", Variant::Primary).clicked() {
                    d.attach(b, host);
                }
                if d.life == Lifecycle::SourceUnavailable && d.app_pid().is_none() && theme::button_compact(ui, "起動する", Variant::Primary).clicked() {
                    let _ = b.send("open_app", json!({ "bundle_id": d.spec.app_identity.bundle_id, "activate": false }));
                }
            }
            Lifecycle::Detached => {
                if theme::button_compact(ui, "閉じる", Variant::Ghost).clicked() {
                    d.close(b);
                    drop_dock = true;
                }
                if theme::button_compact(ui, "Kangae に戻す", Variant::Primary).clicked() {
                    d.attach(b, host);
                }
            }
            _ => {}
        }
        if drop_dock {
            self.dock = None;
            self.slot = None;
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = theme::tokens(ctx);
        let now_t = ctx.input(|i| i.time);
        if let Some(e) = &self.bridge_err {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                theme::label(ui, Type::HeadingMd, "窓を重ねられません", t.text_primary);
                theme::label(ui, Type::Body, e, t.text_secondary);
            });
            return;
        }
        match self.dock.as_ref().map(|d| d.life) {
            None => self.pick(ui, now_t),
            Some(Lifecycle::PermissionRequired) => self.permission(ui),
            Some(_) => self.portal(ui, now_t),
        }
    }

    fn pick(&mut self, ui: &mut egui::Ui, now_t: f64) {
        let t = theme::tokens(ui.ctx());
        let loaded = self.apps.is_some();
        let since = if self.bridge.is_some() { self.loading_since } else { 0.0 };
        let mut chosen = None;
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(24, 20)).show(ui, |ui| {
            ui.set_max_width(560.0);
            theme::label(ui, Type::HeadingMd, "app を選ぶ", t.text_primary);
            ui.add_space(8.0);
            if let Some(n) = &self.note {
                theme::label(ui, Type::Body, n, t.danger);
                ui.add_space(8.0);
            }
            let mut rows: Vec<AppRow> = Vec::new();
            if let Some(apps) = &self.apps {
                if let Some(r) = &self.reaper {
                    let running = apps.iter().find(|a| a.bundle_id == REAPER_ID).cloned();
                    rows.push(running.map(|mut a| {
                        a.version = r.version.clone();
                        a
                    }).unwrap_or_else(|| r.clone()));
                }
                rows.extend(apps.iter().filter(|a| a.bundle_id != REAPER_ID).cloned());
            }
            skeleton::with_skeleton(ui, "apps", loaded, since, Skeleton::List(6), |ui| {
                if rows.is_empty() {
                    theme::label(ui, Type::Body, "起動中の app がありません", t.text_secondary);
                }
                for (i, row) in rows.iter().enumerate() {
                    let resp = ui::motion::fade_slide_in(ui, ("app-row", i), since, |ui| app_row(ui, row, self.open_req.as_ref().is_some_and(|o| o.1.bundle_id == row.bundle_id)));
                    if resp.clicked() {
                        chosen = Some(row.clone());
                    }
                }
            });
        });
        let _ = now_t;
        // 選んだ時だけ bridge を借りる（tuple の中で take すると、選ばない frame でも bridge を落として子 process を殺す）
        let Some(row) = chosen else { return };
        if let Some(mut b) = self.bridge.take() {
            let ctx = ui.ctx().clone();
            self.select(&mut b, row, &ctx);
            self.bridge = Some(b);
        }
    }

    fn permission(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) else { return };
        let mut cancel = false;
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(24, 20)).show(ui, |ui| {
            ui.set_max_width(560.0);
            theme::label(ui, Type::HeadingMd, "アクセシビリティの許可", t.text_primary);
            ui.add_space(4.0);
            theme::label(ui, Type::Body, format!("{}: {}", d.spec.app_identity.name, d.spec.permission_set[0].purpose), t.text_secondary);
            theme::label(
                ui,
                Type::Helper,
                "システム設定 › プライバシーとセキュリティ › アクセシビリティ で Kangae（端末から起動した時はその端末の app）を on にしてください",
                t.text_tertiary,
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if theme::button(ui, "許可を開く", Variant::Primary).clicked() {
                    let _ = b.call("permission", json!({ "prompt": true }), b.limit());
                    let _ = b.call("open_settings", json!({}), b.limit());
                }
                if theme::button(ui, "やめる", Variant::Ghost).clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            if let Ok(to) = d.life.next(LifeEvent::Cancel) {
                d.life = to;
            }
            self.dock = None;
        }
    }

    fn portal(&mut self, ui: &mut egui::Ui, now_t: f64) {
        let t = theme::tokens(ui.ctx());
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click());
        self.slot = Some(rect);
        let covered = self.dock.as_ref().is_some_and(|d| matches!(d.life, Lifecycle::Active | Lifecycle::Resizing));
        // 枠の中の click が Kangae に来た（透明な所を通り抜けなかった）= 元の窓を使いたい → 前に出して focus を渡す
        if covered && resp.clicked() {
            if let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) {
                d.bring_front(b);
            }
        }
        let Some(d) = &self.dock else { return };
        let p = ui.painter();
        if !covered {
            p.rect(rect, 0.0, t.surface_2, Stroke::new(1.0, t.border), StrokeKind::Inside);
        }
        if d.life == Lifecycle::Attaching {
            // 接続中: 枠の形の骨格（帯と光）
            let inset = rect.shrink(24.0);
            for (i, f) in [0.9_f32, 0.6, 0.75, 0.5].iter().enumerate() {
                let y = inset.top() + i as f32 * 28.0;
                skeleton::paint_shimmer(p, Rect::from_min_size(pos2(inset.left(), y), vec2(inset.width() * f, skeleton::LINE_H)), now_t);
            }
            ui.ctx().request_repaint();
        } else if !covered {
            let galley = theme::truncated(ui, &d.reason, Type::Body.font(), t.text_secondary, rect.width() - 48.0);
            ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.text_secondary);
        }
    }

    fn foot(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        ui.horizontal_centered(|ui| {
            let text = match &self.dock {
                Some(d) => format!("{} · 同期: {} · {}", d.spec.status_line(), d.spec.sync_mode.label(), d.metrics.line()),
                None => match &self.bridge {
                    Some(b) => format!("bridge v{} · アクセシビリティ {}", b.hello["version"].as_str().unwrap_or("?"), if b.hello["trusted"].as_bool() == Some(true) { "許可済み" } else { "未許可" }),
                    None => String::new(),
                },
            };
            let galley = theme::truncated(ui, &text, Type::Helper.font(), t.text_tertiary, ui.available_width());
            ui.label(egui::WidgetText::Galley(galley));
        });
    }
}

fn app_row(ui: &mut egui::Ui, row: &AppRow, opening: bool) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    theme::row_background(ui, rect, resp.hovered(), false);
    let state = match (opening, row.pid, &row.version) {
        (true, _, _) => "起動中…".to_string(),
        (false, Some(_), Some(v)) => format!("起動中 · {v}"),
        (false, Some(_), None) => "起動中".to_string(),
        (false, None, Some(v)) => format!("未起動 · {v}"),
        (false, None, None) => "未起動".to_string(),
    };
    let p = ui.painter();
    let name = theme::truncated(ui, &row.name, Type::Body.font(), t.text_primary, rect.width() * 0.6);
    p.galley(pos2(rect.left() + 12.0, rect.center().y - name.size().y / 2.0), name, t.text_primary);
    let st = theme::truncated(ui, &state, Type::Helper.font(), t.text_tertiary, rect.width() * 0.35);
    p.galley(pos2(rect.right() - 12.0 - st.size().x, rect.center().y - st.size().y / 2.0), st, t.text_tertiary);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &row.name));
    resp
}

fn parse_apps(v: &Value, me: u32) -> Vec<AppRow> {
    let mut rows: Vec<AppRow> = v["apps"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|x| x["pid"].as_u64() != Some(me as u64) && !x["bundle_id"].as_str().unwrap_or("").is_empty())
                .map(|x| AppRow {
                    pid: x["pid"].as_u64().map(|p| p as u32),
                    bundle_id: x["bundle_id"].as_str().unwrap_or("").to_string(),
                    name: x["name"].as_str().unwrap_or("").to_string(),
                    version: None,
                    path: x["path"].as_str().map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    rows
}

impl eframe::App for Portal {
    /// 透明な窓の下地（塗らない所は透ける）
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.boot_poll(ctx);
        self.route(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            self.shutdown();
        }
        let t = theme::tokens(ctx);
        egui::TopBottomPanel::top("bar")
            .exact_height(BAR_H)
            .frame(egui::Frame::NONE.fill(t.surface_1).inner_margin(egui::Margin::symmetric(16, 0)))
            .show(ctx, |ui| self.bar(ui, ctx));
        egui::TopBottomPanel::bottom("foot")
            .exact_height(FOOT_H)
            .frame(egui::Frame::NONE.fill(t.surface_1).inner_margin(egui::Margin::symmetric(16, 0)))
            .show(ctx, |ui| self.foot(ui));
        // 重ねている間は中央を塗らない（透明な枠）
        let covered = self.dock.as_ref().is_some_and(|d| matches!(d.life, Lifecycle::Active | Lifecycle::Resizing));
        let fill = if covered { egui::Color32::TRANSPARENT } else { t.bg_app };
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ctx, |ui| self.body(ui, ctx));
        // Kangae の窓の中の click: Kangae の窓が前に出るので、docked の窓を上に戻す（Kangae は active / key のまま）
        if ctx.input(|i| i.pointer.any_pressed()) {
            if let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) {
                d.keep_front(b);
            }
        }
        self.follow(ctx);
        self.log_state();
        self.measure_step(ctx);
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---- 計測（--measure・合成: Kangae の窓を program で動かす。人の drag ではない）----

#[derive(Debug, Clone, Copy)]
enum Step {
    /// 重なるまで待つ（最初の 1 つ）
    Docked,
    Move(f32, f32),
    Resize(f32, f32),
    /// 毎 frame 少しずつ n 回動かす（連続した移動の遅れ）
    Burst(usize, f32),
    /// 自分を前に出す（click の代わり）→ docked の窓が上に戻るか・focus を奪ったか
    Focus,
    /// Kangae を最小化して戻す（hidden → active。元の窓には触らない）
    Minimize,
    /// Kangae を全画面にして戻す（別の Space に移るので degraded になる事を確かめる）
    Fullscreen,
    /// 合わせ直す（degraded → active）
    Redock,
    /// Kangae を最小の大きさにする（枠が元の app の最小より小さければ degraded・元の app は残る）
    Shrink,
    /// Kangae を元の大きさに戻す
    Grow,
    /// 外で開く（元の位置と大きさに戻るか・元の app が前に出るか）
    Handoff,
    /// 崩れる場面を n 回くり返し、毎回 docked の窓が Kangae の窓より前に残るか・Kangae が active / key のままかを数える
    Repeat(Again, usize),
    Write,
}

/// Repeat でくり返す崩れる場面。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Again {
    /// Kangae の窓を前に出す（click の代わり: ViewportCommand::Focus）→ click と同じ口（keep_front）を呼ぶ
    Front,
    /// Kangae の窓を ±40pt 動かす（追従の後に keep_front）
    Move,
    /// Kangae の窓の大きさを ±60pt 変える（追従の後に keep_front）
    Resize,
}

/// KANGAE_MEASURE_PLAN=front: 崩れる場面ごとに 100 回（p-portal 4 本目）
const FRONT_STEPS: &[Step] = &[Step::Docked, Step::Repeat(Again::Front, 100), Step::Repeat(Again::Move, 100), Step::Repeat(Again::Resize, 100), Step::Write];

const STEPS: &[Step] = &[
    Step::Docked,
    Step::Move(40.0, 0.0),
    Step::Move(0.0, 40.0),
    Step::Move(-40.0, -40.0),
    Step::Resize(80.0, 0.0),
    Step::Resize(0.0, 60.0),
    Step::Resize(-80.0, -60.0),
    Step::Burst(20, 6.0),
    Step::Burst(20, -6.0),
    Step::Focus,
    Step::Minimize,
    Step::Fullscreen,
    Step::Redock,
    Step::Shrink,
    Step::Grow,
    Step::Redock,
    Step::Handoff,
    Step::Redock,
    Step::Write,
];

struct Measure {
    out: PathBuf,
    step: usize,
    started: Option<f64>,
    burst_left: usize,
    samples_before: usize,
    log: Vec<Value>,
    failed: Option<String>,
    written: bool,
    host_check: Value,
    front: Value,
    /// 2 段の step（最小化・全画面）の途中の状態と、戻した時刻
    mid: Option<String>,
    flip_at: Option<f64>,
    on_since: Option<f64>,
    resent: bool,
    retry_at: f64,
    /// Repeat の今の回・段・段の始まり（ctx の時刻）と数え
    rep_i: usize,
    rep_phase: u8,
    rep_t0: f64,
    rep: RepCount,
}

#[derive(Debug, Clone, Default)]
struct RepCount {
    /// docked の窓が Kangae の窓より前に残った
    docked_front: usize,
    /// frontmost が Kangae のまま
    kangae_active: usize,
    /// Kangae の窓が key のまま（egui の viewport.focused）
    kangae_key: usize,
    /// 前後を読めなかった
    unread: usize,
    order_calls_before: u32,
    steal_before: u32,
    breaks: Vec<String>,
}

impl Measure {
    fn new(out: PathBuf) -> Measure {
        Measure { out, step: 0, started: None, burst_left: 0, samples_before: 0, log: Vec::new(), failed: None, written: false, host_check: Value::Null, front: Value::Null, mid: None, flip_at: None, on_since: None, resent: false, retry_at: 0.0, rep_i: 0, rep_phase: 0, rep_t0: 0.0, rep: RepCount::default() }
    }
    fn fail(&mut self, why: &str) {
        if self.failed.is_none() {
            self.failed = Some(why.to_string());
        }
    }
}

impl Portal {
    fn measure_step(&mut self, ctx: &egui::Context) {
        let Some(mut m) = self.measure.take() else { return };
        self.measure_inner(ctx, &mut m);
        self.measure = Some(m);
    }

    fn measure_inner(&mut self, ctx: &egui::Context, m: &mut Measure) {
        if m.written {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        let now_t = ctx.input(|i| i.time);
        if m.failed.is_some() {
            return self.measure_write(ctx, m);
        }
        let plan = if std::env::var("KANGAE_MEASURE_PLAN").is_ok_and(|v| v == "front") { FRONT_STEPS } else { STEPS };
        let Some(&step) = plan.get(m.step) else { return };
        // KANGAE_MEASURE_SKIP=fullscreen: 全画面とその後の合わせ直すを飛ばす（全画面の出入りの後に元の窓が AX から長く消える Mac で、残りを測るため）
        if matches!(step, Step::Fullscreen) && std::env::var("KANGAE_MEASURE_SKIP").is_ok_and(|v| v.contains("fullscreen")) {
            m.log.push(json!({ "step": "fullscreen（飛ばした）" }));
            m.step += 2;
            m.started = None;
            return;
        }
        let started = *m.started.get_or_insert(now_t);
        let elapsed = now_t - started;
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        let Some(d) = self.dock.as_ref() else {
            if elapsed > 90.0 {
                m.fail("90 秒待っても app を選べませんでした");
            }
            return;
        };
        let settled = d.tracker.waiting().is_none()
            && match (self.host(ctx).slot, d.tracker.last_target) {
                (Some(s), Some(l)) => s.error_px(&l) < 0.5,
                _ => false,
            };
        let first = elapsed == 0.0;
        match step {
            Step::Docked => {
                // タイル型の窓管理 app と取り合って外れた時は 3 秒ごとに合わせ直す（その間に floating にしてもらう）
                if matches!(d.life, Lifecycle::Degraded) && elapsed >= m.retry_at {
                    m.retry_at = elapsed + 3.0;
                    let host = self.host(ctx);
                    if let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) {
                        d.attach(b, host);
                    }
                    return;
                }
                if matches!(d.life, Lifecycle::Active) && settled {
                    // 座標の確かめ: CG の Kangae の窓の frame と egui の outer_rect
                    if let (Some(b), Some(o)) = (self.bridge.as_mut(), outer) {
                        if let Ok(v) = b.call("cg_windows", json!({ "pid": self.my_pid, "on_screen_only": true }), b.limit()) {
                            let cg = v["windows"].as_array().and_then(|a| a.iter().filter(|w| w["layer"].as_i64() == Some(0)).filter_map(|w| Frame::from_json(&w["frame"])).max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h))));
                            let eg = Frame { x: o.min.x as f64, y: o.min.y as f64, w: o.width() as f64, h: o.height() as f64 };
                            m.host_check = json!({ "cg_frame": cg.map(|f| f.json()), "egui_outer_rect": eg.json(), "error_px": cg.map(|c| c.error_px(&eg)) });
                        }
                    }
                    self.measure_next(m, "docked", elapsed);
                } else if elapsed > 60.0 {
                    m.fail(&format!("60 秒待っても重なりませんでした（{}: {}）", d.life.as_str(), d.reason));
                }
            }
            Step::Move(dx, dy) => {
                if first {
                    m.samples_before = d.metrics.samples.len();
                    if let Some(o) = outer {
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(o.min.x + dx, o.min.y + dy)));
                    }
                } else if (elapsed > 0.3 && settled) || elapsed > 8.0 {
                    self.measure_next(m, &format!("move {dx:+},{dy:+}"), elapsed);
                }
            }
            Step::Resize(dw, dh) => {
                if first {
                    m.samples_before = d.metrics.samples.len();
                    if let Some(i) = inner {
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(i.width() + dw, i.height() + dh)));
                    }
                } else if (elapsed > 0.3 && settled) || elapsed > 8.0 {
                    self.measure_next(m, &format!("resize {dw:+},{dh:+}"), elapsed);
                }
            }
            Step::Burst(n, dx) => {
                if first {
                    m.samples_before = d.metrics.samples.len();
                    m.burst_left = n;
                }
                if m.burst_left > 0 {
                    if let Some(o) = outer {
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(o.min.x + dx, o.min.y)));
                    }
                    m.burst_left -= 1;
                } else if (settled && elapsed > 0.3) || elapsed > 15.0 {
                    self.measure_next(m, &format!("burst {n}×{dx:+}"), elapsed);
                }
            }
            Step::Focus => {
                if first {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                } else if elapsed > 2.0 {
                    if let Some(b) = self.bridge.as_mut() {
                        m.front = b.call("frontmost", json!({}), b.limit()).unwrap_or(Value::Null);
                    }
                    self.measure_next(m, "focus（自分を前に出す）", elapsed);
                }
            }
            Step::Minimize | Step::Fullscreen => {
                let mini = matches!(step, Step::Minimize);
                let on = |v: bool| if mini { egui::ViewportCommand::Minimized(v) } else { egui::ViewportCommand::Fullscreen(v) };
                // 今その状態か（全画面の出入りは animation が長い。混んだ機械ではもっと）
                let now_on = ctx.input(|i| if mini { i.viewport().minimized } else { i.viewport().fullscreen }).unwrap_or(false);
                if first {
                    m.mid = None;
                    m.flip_at = None;
                    m.on_since = None;
                    m.resent = false;
                    ctx.send_viewport_cmd(on(true));
                } else if m.mid.is_none() && now_on && m.on_since.is_none() {
                    m.on_since = Some(elapsed);
                } else if m.mid.is_none() && (m.on_since.is_some_and(|t| elapsed - t > 3.0) || elapsed > 15.0) {
                    m.mid = Some(format!("{}{}: {}", if now_on { "" } else { "（入れなかった）" }, d.life.as_str(), d.reason));
                    m.flip_at = Some(elapsed);
                    ctx.send_viewport_cmd(on(false));
                } else if m.mid.is_some() && now_on && !m.resent && elapsed - m.flip_at.unwrap_or(0.0) > 6.0 {
                    // 出入りの animation の途中に戻すと無視される（"not in fullscreen state"）ので 1 回だけ送り直す
                    m.resent = true;
                    ctx.send_viewport_cmd(on(false));
                } else if m.mid.is_some() && ((!now_on && elapsed - m.flip_at.unwrap_or(0.0) > 2.5) || elapsed - m.flip_at.unwrap_or(0.0) > 15.0) {
                    let name = if matches!(step, Step::Minimize) { "minimize → 戻す" } else { "fullscreen → 戻す" };
                    self.measure_next(m, name, elapsed);
                }
            }
            Step::Redock => {
                // 全画面から戻った直後は元の窓がまだ今の Space に無い事がある → 2 秒ごとに試し直す
                if matches!(d.life, Lifecycle::Degraded | Lifecycle::Detached) && elapsed >= m.retry_at {
                    m.retry_at = elapsed + 2.0;
                    let host = self.host(ctx);
                    if let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) {
                        d.attach(b, host);
                    }
                } else if (matches!(d.life, Lifecycle::Active) && settled) || elapsed > 30.0 {
                    self.measure_next(m, "合わせ直す", elapsed);
                }
            }
            Step::Shrink | Step::Grow => {
                if first {
                    let size = if matches!(step, Step::Shrink) { vec2(720.0, 480.0) } else { vec2(1280.0, 800.0) };
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                } else if (elapsed > 1.0 && (settled || matches!(d.life, Lifecycle::Degraded))) || elapsed > 10.0 {
                    m.mid = Some(format!("{}: {}", d.life.as_str(), d.reason));
                    self.measure_next(m, if matches!(step, Step::Shrink) { "Kangae を最小に（720×480）" } else { "Kangae を戻す（1280×800）" }, elapsed);
                }
            }
            Step::Handoff => {
                if first {
                    if let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) {
                        if let Err(e) = d.handoff(b) {
                            m.mid = Some(format!("外で開けない: {e}"));
                        }
                    }
                } else if d.life == Lifecycle::Detached || elapsed > 15.0 {
                    // 元の位置と大きさに戻ったかを AX で読み戻す
                    let (handle, original) = (d.window.as_ref().map(|w| w.handle.clone()), d.original);
                    if let (Some(h), Some(o), Some(b)) = (handle, original, self.bridge.as_mut()) {
                        let back = b.call("frame", json!({ "handle": h }), b.limit()).ok().and_then(|v| Frame::from_json(&v["frame"]));
                        let front = b.call("frontmost", json!({}), b.limit()).ok().and_then(|v| v["pid"].as_u64());
                        m.mid = Some(format!(
                            "{}: 元の frame {} / 今 {} / 差 {} / frontmost pid {:?}（元の app pid {:?}）",
                            d.life.as_str(),
                            o.text(),
                            back.map(|f| f.text()).unwrap_or_default(),
                            back.map(|f| format!("{:.1}pt", f.error_px(&o))).unwrap_or_else(|| "読めない".into()),
                            front,
                            d.app_pid()
                        ));
                    }
                    self.measure_next(m, "外で開く", elapsed);
                }
            }
            Step::Repeat(kind, n) => {
                // KANGAE_MEASURE_REPEAT=N で回数を変える（負の対照を短く回す時）
                let n = std::env::var("KANGAE_MEASURE_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(n);
                if first {
                    m.rep_i = 0;
                    m.rep_phase = 0;
                    m.rep_t0 = now_t;
                    m.rep = RepCount { order_calls_before: d.metrics.order_calls, steal_before: d.metrics.focus_steal_events, ..RepCount::default() };
                }
                if m.rep_i >= n {
                    let r = std::mem::take(&mut m.rep);
                    let calls = d.metrics.order_calls - r.order_calls_before;
                    let steals = d.metrics.focus_steal_events - r.steal_before;
                    m.mid = Some(format!(
                        "{n} 回: docked の窓が前に残った {} / Kangae が active のまま {} / key のまま {} / 前後を読めない {} / 真下に置いた {calls} 回 / focus 奪取 {steals}{}",
                        r.docked_front,
                        r.kangae_active,
                        r.kangae_key,
                        r.unread,
                        if r.breaks.is_empty() { String::new() } else { format!(" / 崩れた回: {}", r.breaks.join("・")) }
                    ));
                    let name = match kind {
                        Again::Front => format!("Kangae を前に出す（click の代わり）× {n}"),
                        Again::Move => format!("Kangae の窓を動かす × {n}"),
                        Again::Resize => format!("Kangae の大きさを変える × {n}"),
                    };
                    return self.measure_next(m, &name, elapsed);
                }
                let since = now_t - m.rep_t0;
                let sign = if m.rep_i % 2 == 0 { 1.0 } else { -1.0 };
                match (m.rep_phase, kind) {
                    (0, Again::Front) => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        m.rep_phase = 1;
                        m.rep_t0 = now_t;
                    }
                    (1, Again::Front) if since > 0.15 => {
                        // click と同じ口（update の pointer の press が呼ぶ物）
                        if let (Some(d), Some(b)) = (self.dock.as_mut(), self.bridge.as_mut()) {
                            d.keep_front(b);
                        }
                        m.rep_phase = 2;
                        m.rep_t0 = now_t;
                    }
                    (0, Again::Move) => {
                        if let Some(o) = outer {
                            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(o.min.x + 40.0 * sign, o.min.y)));
                        }
                        m.rep_phase = 2;
                        m.rep_t0 = now_t;
                    }
                    (0, Again::Resize) => {
                        if let Some(i) = inner {
                            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(i.width() + 60.0 * sign, i.height())));
                        }
                        m.rep_phase = 2;
                        m.rep_t0 = now_t;
                    }
                    (2, _) if (since > 0.45 && settled && self.dock.as_ref().is_some_and(|d| d.tracker.waiting().is_none())) || since > 6.0 => {
                        // 読む: CG の前後（前から後ろ）・frontmost・Kangae の窓が key か
                        let key = ctx.input(|i| i.viewport().focused).unwrap_or(false);
                        let (cg, me) = (self.dock.as_ref().and_then(|d| d.window.as_ref()).and_then(|w| w.cg_id), self.my_pid as u64);
                        if let Some(b) = self.bridge.as_mut() {
                            let lim = b.limit();
                            let z = b.call("cg_windows", json!({ "on_screen_only": true }), lim).ok();
                            let front = b.call("frontmost", json!({}), lim).ok().and_then(|v| v["pid"].as_u64());
                            let list = z.as_ref().and_then(|v| v["windows"].as_array().cloned()).unwrap_or_default();
                            let docked = list.iter().filter(|w| w["id"].as_i64() == cg && cg.is_some()).filter_map(|w| w["order"].as_u64()).min();
                            let host = list.iter().filter(|w| w["pid"].as_u64() == Some(me) && w["layer"].as_i64() == Some(0)).filter_map(|w| w["order"].as_u64()).min();
                            match (docked, host) {
                                (Some(dk), Some(h)) if dk < h => m.rep.docked_front += 1,
                                (Some(dk), Some(h)) => m.rep.breaks.push(format!("{}回目 docked {dk} > Kangae {h}", m.rep_i + 1)),
                                _ => m.rep.unread += 1,
                            }
                            if front == Some(me) {
                                m.rep.kangae_active += 1;
                            }
                        }
                        if key {
                            m.rep.kangae_key += 1;
                        }
                        m.rep_i += 1;
                        m.rep_phase = 0;
                        m.rep_t0 = now_t;
                    }
                    _ => {}
                }
            }
            Step::Write => self.measure_write(ctx, m),
        }
    }

    fn measure_next(&mut self, m: &mut Measure, name: &str, elapsed: f64) {
        if let Some(d) = &self.dock {
            let new: Vec<Value> = d.metrics.samples.iter().skip(m.samples_before).map(|s| json!({ "error_px": s.error_px, "lag_ms": s.lag_ms })).collect();
            m.log.push(json!({
                "step": name,
                "seconds": elapsed,
                "state": d.life.as_str(),
                "mid_state": m.mid.take(),
                "reason": d.reason,
                "samples": new,
                "focus_steal_events": d.metrics.focus_steal_events,
                "z_order_checks": d.metrics.z_order_checks,
                "z_order_breaks": d.metrics.z_order_breaks,
            }));
        }
        m.step += 1;
        m.started = None;
    }

    fn measure_write(&mut self, ctx: &egui::Context, m: &mut Measure) {
        m.written = true;
        let (ppp, monitor) = ctx.input(|i| (i.viewport().native_pixels_per_point, i.viewport().monitor_size));
        let dock = self.dock.as_ref();
        let result = json!({
            "kind": "合成（Kangae の窓を program の ViewportCommand で動かした・人の drag ではない）",
            "failed": m.failed,
            "env": {
                "os": sh("/usr/bin/sw_vers", &["-productVersion"]),
                "model": sh("/usr/sbin/sysctl", &["-n", "hw.model"]),
                "loadavg": sh("/usr/sbin/sysctl", &["-n", "vm.loadavg"]),
                "native_pixels_per_point": ppp,
                "monitor_size": monitor.map(|s| [s.x, s.y]),
                "bridge": self.bridge.as_ref().map(|b| b.hello.clone()),
            },
            "app": dock.map(|d| serde_json::to_value(&d.spec.app_identity).unwrap_or(Value::Null)),
            "window": dock.and_then(|d| d.window.as_ref()).map(|w| json!({ "title": w.title, "cg_id": w.cg_id, "frame": w.frame.map(|f| f.json()) })),
            "original_frame": dock.and_then(|d| d.original).map(|f| f.json()),
            "host_coord_check": m.host_check,
            "frontmost_after_focus": m.front,
            "my_pid": self.my_pid,
            "tiler": self.tiler,
            "steps": m.log,
            "summary": dock.map(|d| serde_json::to_value(d.metrics.summary()).unwrap_or(Value::Null)),
            "focus_steal_events": dock.map(|d| d.metrics.focus_steal_events),
            "z_order_checks": dock.map(|d| d.metrics.z_order_checks),
            "z_order_breaks": dock.map(|d| d.metrics.z_order_breaks),
            "human_moves": dock.map(|d| d.metrics.human_moves),
            "timeouts": dock.map(|d| d.metrics.timeouts),
            "order_calls": dock.map(|d| d.metrics.order_calls),
            "order_failures": dock.map(|d| d.metrics.order_failures),
            "capabilities": dock.map(|d| serde_json::to_value(&d.spec.capabilities).unwrap_or(Value::Null)),
            "sync_mode": dock.map(|d| d.spec.sync_mode.label()),
            "final_state": dock.map(|d| d.life.as_str()),
            "notes": dock.map(|d| d.metrics.notes.clone()),
        });
        match std::fs::write(&m.out, serde_json::to_string_pretty(&result).unwrap_or_default()) {
            Ok(()) => println!("計測を書きました: {}", m.out.display()),
            Err(e) => eprintln!("{} に書けません: {e}", m.out.display()),
        }
        self.shutdown();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn sh(bin: &str, args: &[&str]) -> String {
    Command::new(bin).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

// ---- probe（窓を動かさない）----

fn probe(bundle: &str) -> i32 {
    let b = surface::find_bridge().and_then(|p| Bridge::spawn(&p, surface::BRIDGE_READY_LIMIT));
    let mut b = match b {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("bridge: {}（{}）", b.hello, b.path.display());
    let lim = b.limit();
    match b.call("find_app", json!({ "bundle_id": bundle }), lim) {
        Ok(v) => println!("find_app: {v}"),
        Err(e) => println!("find_app: {e}"),
    }
    let apps = match b.call("apps", json!({ "bundle_id": bundle, "regular_only": false }), lim) {
        Ok(v) => v,
        Err(e) => {
            println!("apps: {e}");
            return 1;
        }
    };
    let pids: Vec<u64> = apps["apps"].as_array().map(|a| a.iter().filter_map(|x| x["pid"].as_u64()).collect()).unwrap_or_default();
    println!("apps: {} 件 {:?}", pids.len(), pids);
    for pid in pids {
        match b.call("windows", json!({ "pid": pid }), lim) {
            Ok(v) => {
                for w in surface::parse_windows(&v) {
                    println!(
                        "  {} 「{}」 {} main={} min={} full={} movable={} frame={} cg={:?}",
                        w.handle,
                        w.title,
                        w.subrole,
                        w.main,
                        w.minimized,
                        w.fullscreen,
                        w.movable,
                        w.frame.map(|f| f.text()).unwrap_or_default(),
                        w.cg_id
                    );
                }
                println!("  pick: {}", surface::pick_main_window(&surface::parse_windows(&v)).map(|w| format!("{}「{}」", w.handle, w.title)).unwrap_or_else(|e| e));
            }
            Err(e) => println!("  windows pid {pid}: {e}（{}）", e.code()),
        }
    }
    0
}

// ---- 証拠（REAPER の状態の読み戻しと .rpp・読むだけ）----

/// REAPER の中で走る Lua: 今の project の file・未保存か・最後の undo・item の一覧を返す（何も変えない）。
const STATE_LUA: &str = r#"local ok, err = pcall(function()
  local _, fn = reaper.EnumProjects(-1)
  local out = {"proj\t" .. (fn or "") .. "\t" .. tostring(reaper.IsProjectDirty(0)) .. "\t" .. (reaper.Undo_CanUndo2(0) or "")}
  for i = 0, reaper.CountMediaItems(0) - 1 do
    local item = reaper.GetMediaItem(0, i)
    local tr = reaper.GetMediaItem_Track(item)
    local tname = ""
    if tr then local _, n = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false); tname = n end
    local take = reaper.GetActiveTake(item)
    local takename = take and reaper.GetTakeName(take) or ""
    out[#out + 1] = table.concat({"item", guid(item), reaper.GetMediaItemInfo_Value(item, "D_POSITION"),
      reaper.GetMediaItemInfo_Value(item, "D_LENGTH"), takename, tname}, "\t")
  end
  report(table.concat(out, "\n"))
end)
if not ok then report("err\t" .. tostring(err)) end
"#;

fn evidence_dir() -> PathBuf {
    surface::data_dir().join("portal-evidence") // FX1: app と同じ data dir
}

fn reaper_state(timeout: Duration) -> Result<surface::ReaperState, String> {
    let pid = audio::surface::pid_of(audio::surface::REAPER.process).ok_or("REAPER が起動していません（読み戻しは起動中の REAPER に聞きます）")?;
    let bin = std::env::var_os("KANGAE_REAPER_BIN")
        .map(PathBuf::from)
        .or_else(|| audio::surface::exe_of(pid))
        .unwrap_or_else(|| PathBuf::from("/Applications/REAPER.app/Contents/MacOS/REAPER"));
    let text = audio::reaper::send(&bin, |done, cancel| audio::reaper::script("", done, cancel, STATE_LUA), timeout)?
        .ok_or_else(|| format!("REAPER が {} 秒以内に答えませんでした（dialog が開いていないか見てください）", timeout.as_secs()))?;
    surface::parse_reaper_state(&text)
}

fn rpp_snapshot(path: &Path) -> Value {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64());
            json!({ "path": path.display().to_string(), "mtime": mtime, "items": surface::rpp_items(&text) })
        }
        Err(e) => json!({ "path": path.display().to_string(), "error": format!("読めません: {e}") }),
    }
}

fn evidence(args: &[String]) -> i32 {
    let timeout = Duration::from_secs(std::env::var("KANGAE_REAPER_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(90));
    match args {
        [cmd, name, rest @ ..] if cmd == "save" => {
            if name.is_empty() || name.contains('/') || name.starts_with('.') {
                eprintln!("NAME は / を含まない名前で（例: before）");
                return 2;
            }
            let rpp_arg = match rest {
                [] => None,
                [flag, p] if flag == "--rpp" => Some(PathBuf::from(p)),
                _ => {
                    eprintln!("{USAGE}");
                    return 2;
                }
            };
            let state = reaper_state(timeout);
            let rpp = rpp_arg.or_else(|| state.as_ref().ok().filter(|s| !s.project.is_empty()).map(|s| PathBuf::from(&s.project)));
            let snap = json!({
                "name": name,
                "at_ms": surface::now_ms(),
                "reaper": match &state { Ok(s) => serde_json::to_value(s).unwrap_or(Value::Null), Err(e) => json!({ "error": e }) },
                "rpp": rpp.as_deref().map(rpp_snapshot),
            });
            let dir = evidence_dir();
            let file = dir.join(format!("{name}.json"));
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, serde_json::to_string_pretty(&snap).unwrap_or_default())) {
                eprintln!("{} に書けません: {e}", file.display());
                return 1;
            }
            match &state {
                Ok(s) => println!(
                    "REAPER: {}・{}・item {} 個・最後の操作「{}」",
                    if s.project.is_empty() { "未保存の project" } else { &s.project },
                    if s.dirty { "未保存の変更あり" } else { "保存済み" },
                    s.items.len(),
                    s.last_undo
                ),
                Err(e) => println!("REAPER: {e}"),
            }
            match &snap["rpp"] {
                Value::Null => println!(".rpp: 無し（project が未保存。--rpp FILE で指定）"),
                r => println!(".rpp: {}・item {} 個", r["path"].as_str().unwrap_or(""), r["items"].as_array().map(|a| a.len()).unwrap_or(0)),
            }
            println!("写し: {}", file.display());
            if state.is_ok() { 0 } else { 1 }
        }
        [cmd, a, b] if cmd == "diff" => {
            let load = |n: &str| -> Result<Value, String> {
                let f = evidence_dir().join(format!("{n}.json"));
                let text = std::fs::read_to_string(&f).map_err(|e| format!("{} を読めません: {e}", f.display()))?;
                serde_json::from_str(&text).map_err(|e| format!("{} が壊れています: {e}", f.display()))
            };
            let (a, b) = match (load(a), load(b)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            let items = |v: &Value| -> Option<Vec<surface::RppItem>> { serde_json::from_value(v["items"].clone()).ok() };
            println!("REAPER の状態（開いている project の中身）:");
            match (items(&a["reaper"]), items(&b["reaper"])) {
                (Some(x), Some(y)) => {
                    let d = surface::diff_items(&x, &y);
                    println!("  {}", if d.is_empty() { "変わっていません".to_string() } else { d.join("\n  ") });
                    println!("  未保存の変更: {} → {}", a["reaper"]["dirty"], b["reaper"]["dirty"]);
                    println!("  最後の操作: 「{}」→「{}」", a["reaper"]["last_undo"].as_str().unwrap_or(""), b["reaper"]["last_undo"].as_str().unwrap_or(""));
                }
                _ => println!("  比べられません（どちらかの写しに REAPER の状態が無い）"),
            }
            println!(".rpp（保存 file）:");
            let changed_file = match (items(&a["rpp"]), items(&b["rpp"])) {
                (Some(x), Some(y)) => {
                    let d = surface::diff_items(&x, &y);
                    println!("  {}", if d.is_empty() { "変わっていません".to_string() } else { d.join("\n  ") });
                    println!("  mtime: {} → {}", a["rpp"]["mtime"], b["rpp"]["mtime"]);
                    !d.is_empty()
                }
                _ => {
                    println!("  比べられません（どちらかの写しに .rpp が無い）");
                    false
                }
            };
            if changed_file { 0 } else { 1 }
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

// ---- WebView の面（v0.14 §3.1・SurfaceBackend::WebView・図 §18 の WebView の段）----

/// `web` の引数
struct WebArgs {
    spec: surface::WebSpec,
    quit_after: Option<f64>,
    store_test: bool,
}

/// 自前の頁（127.0.0.1）で data store を確かめる時の port（localStorage は port まで origin なので固定）
const STORE_PORT: u16 = 47833;
#[cfg(all(feature = "webview", target_os = "macos"))]
const STORE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>store</title><body style=\"font:16px -apple-system,sans-serif;margin:24px\"><p id=r></p>\
<script>const b=localStorage.getItem('kangae');if(!b)localStorage.setItem('kangae',new Date().toISOString());\
const t='store '+(b?'kept '+b:'empty');document.title=t;document.getElementById('r').textContent=t;</script>";

fn parse_web(args: &[String]) -> Result<WebArgs, String> {
    let mut a = WebArgs { spec: surface::WebSpec::default(), quit_after: None, store_test: false };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        let mut val = || it.next().cloned().ok_or_else(|| format!("{x} に値が要ります"));
        match x.as_str() {
            "--app" => a.spec.software_id = val()?,
            "--space" => a.spec.space_id = val()?,
            "--allow" => a.spec.allow_hosts = val()?.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect(),
            "--youtube" => a.spec.youtube = Some(val()?),
            "--no-referer" => a.spec.no_referer = true,
            "--quit-after" => a.quit_after = Some(val()?.parse().map_err(|_| "--quit-after は秒で".to_string())?),
            "--store-test" => a.store_test = true,
            u if !u.starts_with("--") && a.spec.url.is_empty() => a.spec.url = u.to_string(),
            other => return Err(format!("不明な引数: {other}")),
        }
    }
    if a.store_test {
        a.spec.url = format!("http://127.0.0.1:{STORE_PORT}/store.html");
        a.spec.allow_hosts = vec!["127.0.0.1".into()];
    }
    if a.spec.youtube.is_some() && a.spec.allow_hosts.is_empty() {
        a.spec.allow_hosts = vec!["youtube.com".into()];
    }
    if a.spec.allow_hosts.is_empty() {
        a.spec.allow_hosts = surface::host_of(&a.spec.url).into_iter().collect();
    }
    if a.spec.software_id.is_empty() {
        a.spec.software_id = surface::host_of(&a.spec.url).unwrap_or_else(|| "web".into());
    }
    if a.spec.space_id.is_empty() {
        a.spec.space_id = "default".into();
    }
    if a.spec.url.is_empty() && a.spec.youtube.is_none() {
        return Err("開く URL が要ります".into());
    }
    Ok(a)
}

fn web_log(line: &str) {
    println!("{line}");
    if let Some(p) = std::env::var_os("KANGAE_PORTAL_LOG") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{:.0}\t{line}", surface::now_ms());
        }
    }
}

fn web(args: &[String]) -> i32 {
    let a = match parse_web(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return 2;
        }
    };
    web_run(a)
}

/// feature webview の無い build: WebView は作らず、本人の browser へ（handoff）
#[cfg(not(all(feature = "webview", target_os = "macos")))]
fn web_run(a: WebArgs) -> i32 {
    let url = match a.spec.target() {
        Ok(u) => u,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    web_log(&format!("state\tintegration_failed\t{}。本人の browser で開きます: {url}", surface::WEBVIEW_MISSING));
    match surface::open_in_browser(&url) {
        Ok(()) => {
            web_log(&format!("handoff\t{url}"));
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(all(feature = "webview", target_os = "macos"))]
fn web_run(a: WebArgs) -> i32 {
    if a.store_test {
        std::thread::spawn(serve_store_page);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Kangae Portal").with_inner_size([1100.0, 760.0]).with_min_inner_size([480.0, 360.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Kangae Portal",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx, egui::ThemePreference::System);
            Ok(Box::new(WebApp::new(a)))
        }),
    );
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("窓を開けません: {e}");
            1
        }
    }
}

/// 自前の頁を 127.0.0.1 で配る（data store の確かめだけ。頁の script は頁自身の物で、Kangae は script を入れない）
#[cfg(all(feature = "webview", target_os = "macos"))]
fn serve_store_page() {
    use std::io::{Read as _, Write as _};
    let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", STORE_PORT)) else {
        return web_log(&format!("note\t127.0.0.1:{STORE_PORT} を開けません（別の試験が使っている）"));
    };
    for mut stream in listener.incoming().flatten() {
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let body = STORE_PAGE.as_bytes();
        let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body));
    }
}

#[cfg(all(feature = "webview", target_os = "macos"))]
struct WebApp {
    args: WebArgs,
    surface: Option<surface::webview::WebSurface>,
    failed: Option<String>,
    started: f64,
    last: String,
    notes_logged: usize,
}

#[cfg(all(feature = "webview", target_os = "macos"))]
impl WebApp {
    fn new(args: WebArgs) -> WebApp {
        WebApp { args, surface: None, failed: None, started: -1.0, last: String::new(), notes_logged: 0 }
    }

    fn portal_state(&self) -> (ui::portal_frame::PortalState, String) {
        use surface::WebState;
        use ui::portal_frame::PortalState;
        if let Some(why) = &self.failed {
            return (PortalState::IntegrationFailed, why.clone());
        }
        match self.surface.as_ref().map(|s| &s.life.state) {
            None | Some(WebState::Starting | WebState::Loading) => (PortalState::Attaching, String::new()),
            Some(WebState::Live) => (PortalState::NativeDirect, self.surface.as_ref().and_then(|s| s.life.detail.clone()).unwrap_or_default()),
            Some(WebState::Recoverable(why)) => (PortalState::IntegrationFailed, why.clone()),
        }
    }

    fn handoff(&mut self) {
        let url = self.surface.as_ref().map(|s| s.handoff_url()).or_else(|| self.args.spec.target().ok()).unwrap_or_default();
        if let Some(s) = self.surface.as_mut() {
            s.hide();
        }
        match surface::open_in_browser(&url) {
            Ok(()) => web_log(&format!("handoff\t{url}")),
            Err(e) => web_log(&format!("handoff_failed\t{e}")),
        }
    }
}

#[cfg(all(feature = "webview", target_os = "macos"))]
impl eframe::App for WebApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        use ui::portal_frame::{self, PortalAction, PortalFrame, SyncMode};
        let now_t = ctx.input(|i| i.time);
        if self.started < 0.0 {
            self.started = now_t;
        }
        let (state, detail) = self.portal_state();
        let name = self.args.spec.youtube.as_ref().map(|_| "YouTube".to_string()).unwrap_or_else(|| self.args.spec.software_id.clone());
        let t = theme::tokens(ctx);
        let mut slot = None;
        let mut action = None;
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.bg_app).inner_margin(egui::Margin::same(12))).show(ctx, |ui| {
            let r = portal_frame::show(ui, "web", &PortalFrame { app_name: &name, state, sync: SyncMode::VisualLive, mode: None, detail: &detail }, |_| {});
            slot = Some(r.slot);
            action = r.action;
        });
        let rect = slot.map(|r| Frame { x: r.min.x as f64, y: r.min.y as f64, w: r.width() as f64, h: r.height() as f64 });
        // 最初の frame で開く（人がこの面を開いた結果。裏で作り置きしない）
        if self.surface.is_none() && self.failed.is_none() {
            if let Some(r) = rect {
                match surface::webview::WebSurface::open(frame, self.args.spec.clone(), r) {
                    Ok(s) => {
                        web_log(&format!("open\t{}\tstore {}", s.life.url, s.store_id.iter().map(|b| format!("{b:02x}")).collect::<String>()));
                        self.surface = Some(s);
                    }
                    Err(e) => self.failed = Some(e),
                }
            }
        }
        // egui の popup が開いている間は WebView を隠す（子 NSView は egui より上に乗る）
        let cover = ctx.memory(|m| m.any_popup_open());
        if let Some(s) = self.surface.as_mut() {
            if s.frame(rect.map(|r| (r, r)), cover, &[], false) {
                // first responder が WebView の中: egui の focus を外す（枠が二重に出ない）
                ctx.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
            while self.notes_logged < s.notes.len() {
                web_log(&format!("note\t{}", s.notes[self.notes_logged]));
                self.notes_logged += 1;
            }
        }
        let (state, detail) = self.portal_state();
        let title = self.surface.as_ref().map(|s| s.life.title.clone()).unwrap_or_default();
        let line = format!("state\t{state:?}\t{detail}\ttitle {title}");
        if line != self.last {
            web_log(&line);
            self.last = line;
        }
        match action {
            Some(PortalAction::ContinueInSource) | Some(PortalAction::Detach) => self.handoff(),
            Some(PortalAction::Retry) => {
                self.surface = None;
                self.failed = None;
            }
            _ => {}
        }
        if let Some(q) = self.args.quit_after {
            if now_t - self.started > q {
                web_log("quit");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
