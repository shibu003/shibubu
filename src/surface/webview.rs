//! `SurfaceBackend::WebView` の面（v0.14 §3.1・CD-24〜26）。feature `webview` の時だけ compile する（wry 0.57・Cargo.toml に足した crate はこれだけ）。
//! eframe の窓の NSView に WKWebView を子で置く（`build_as_child`）。Kangae は矩形を予約するだけで、入力・IME・file panel は WebView が直接受ける。
//! 守る事（CD-25）: data store は Browser の面が Shibubu で 1 つ（W12W-2・`SHARED_WEB_STORE`・§3）・Browser でない面（Diagram）は Project ごと（G72・`project::web::store_of`）・cookie と web storage を読まない・第三者の頁に script を入れない（`evaluate` は `may_evaluate` が通した許可の host の頁だけ・文書の開始に入れるのは `report_visible` の面〈Browser でない〉の `REPORT_VISIBLE_JS` だけ）・IPC を登録しない・
//! navigation は許可の host だけ（sign-in / OAuth は handoff）・新しい窓と camera / mic は deny・download は `WebSpec::download_dir` が在る時だけその直下へ（無ければ deny）・devtools は debug build だけ。
//! YouTube は /embed/<id> を top-level で開き Referer を付ける（CD-26）。判定は純関数（mod.rs の「WebView の面」）で、ここは wry との糊だけ。図は §18。

use super::{decide_navigation, staged_download_path, store_identifier, AppKitOp, FocusFix, Frame, WebEvent, WebFault, WebLife, WebNav, WebSpec, WebState, KANGAE_REFERER};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Instant;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::raw_window_handle::HasWindowHandle;
use wry::{NewWindowResponse, PageLoadEvent, PermissionKind, PermissionResponse, Rect, WebView, WebViewBuilder, WebViewBuilderExtDarwin, WebViewExtMacOS};

/// eframe の窓の中の WebView 1 つ。
/// Wave: W12W-0
pub struct WebSurface {
    view: WebView,
    /// wry の handler が送った event と、送った時の generation（`gen`）。`frame` が `WebLife::on` に渡す（前の generation の物は捨てられる）
    events: mpsc::Receiver<(u64, WebEvent)>,
    /// handler が event に付ける generation（`retry` が進める）
    gen: Arc<AtomicU64>,
    pub life: WebLife,
    pub spec: WebSpec,
    pub store_id: [u8; 16],
    /// 最後に置いた (面の矩形の全体, 見えている所)
    applied: Option<(Frame, Frame)>,
    /// MIRROR-FIT-1101 追記 210: 箱に抜いた穴
    holes: Holes,
    shown: bool,
    /// AX の木に WebView を足した結果・Edit menu の結果（記録の 1 行）
    pub notes: Vec<String>,
    patched: bool,
}

impl WebSurface {
    /// 開く（人の操作で 1 回。WebView を作ると app が前に出るので、裏で作り置きしない）。開けない・開いてはいけない時は Err（理由）= 呼ぶ側が handoff。
    pub fn open<W: HasWindowHandle>(parent: &W, spec: WebSpec, rect: Frame) -> Result<WebSurface, String> {
        let url = spec.target()?;
        if let WebNav::Deny { why, .. } = decide_navigation(&url, &spec.allow_hosts) {
            return Err(why);
        }
        let store_id = store_identifier(&spec.space_id, &spec.software_id);
        let (tx, rx) = mpsc::channel::<(u64, WebEvent)>();
        let gen = Arc::new(AtomicU64::new(0));
        // handler ごとの送り口: 送った時の generation を付ける（retry の前の読み込みの event を WebLife::on が捨てられるように）
        let sender = || {
            let (tx, gen) = (tx.clone(), gen.clone());
            move |ev: WebEvent| {
                let _ = tx.send((gen.load(Ordering::SeqCst), ev));
            }
        };
        let allow = spec.allow_hosts.clone();
        let (t_load, t_nav, t_win, t_title, t_dl, t_done, t_gone) = (sender(), sender(), sender(), sender(), sender(), sender(), sender());
        let dl_dir = spec.download_dir.clone();
        let mut builder = WebViewBuilder::new()
            .with_bounds(bounds(rect))
            .with_visible(false) // 最初の読み込みが終わるまで隠して skeleton（DESIGN §15.2-2）
            .with_devtools(cfg!(debug_assertions))
            .with_data_store_identifier(store_id)
            .with_on_page_load_handler(move |event, url| {
                t_load(match event {
                    PageLoadEvent::Started => WebEvent::Started(url),
                    PageLoadEvent::Finished => WebEvent::Finished(url),
                });
            })
            .with_navigation_handler(move |url| match decide_navigation(&url, &allow) {
                WebNav::Allow => true,
                WebNav::Deny { handoff, why } => {
                    t_nav(WebEvent::Blocked { url, handoff, why });
                    false
                }
            })
            .with_new_window_req_handler(move |url, _features| {
                t_win(WebEvent::NewWindow(url));
                NewWindowResponse::Deny
            })
            .with_document_title_changed_handler(move |title| {
                t_title(WebEvent::Title(title));
            })
            // `<a download>` は wry が navigation handler より先に download に回す（wry-0.57.0/src/wkwebview/navigation.rs:68）。
            // download_dir が無ければ今まで通り止める・在ればその直下に落とす（名前と repo の置き場は Shibubu の inline の欄で決める）
            .with_download_started_handler(move |url, path| match &dl_dir {
                Some(dir) => {
                    let _ = std::fs::create_dir_all(dir); // 書けなければ WebKit が download_completed の ok=false で言う
                    *path = staged_download_path(dir, path);
                    t_dl(WebEvent::DownloadStaged { url, staged: path.clone() });
                    true
                }
                None => {
                    t_dl(WebEvent::Download(url));
                    false
                }
            })
            .with_download_completed_handler(move |url, _path, ok| {
                t_done(WebEvent::DownloadDone { url, ok });
            })
            // camera / mic など全部 deny（CD-25 (5)）
            .with_permission_handler(|_kind: PermissionKind| PermissionResponse::Deny)
            .with_on_web_content_process_terminate_handler(move || {
                t_gone(WebEvent::ProcessGone);
            });
        if spec.report_visible {
            builder = builder.with_initialization_script_for_main_only(super::REPORT_VISIBLE_JS, true);
        }
        builder = match referer(&spec) {
            Some(headers) => builder.with_url_and_headers(url.clone(), headers),
            None => builder.with_url(url.clone()),
        };
        let view = builder.build_as_child(parent).map_err(|e| format!("Couldn't create the web view: {e}"))?;
        let youtube = spec.youtube.is_some();
        let mut life = WebLife::new(&url, youtube, Instant::now());
        // start-once = この process で最初に開いた面の generation 0 の 1 回だけ（閉じて開き直した面にまた当てない）
        static FAULT_USED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        life.fault = WebFault::from_env().filter(|_| !FAULT_USED.swap(true, Ordering::SeqCst));
        Ok(WebSurface {
            view,
            events: rx,
            gen,
            life,
            spec,
            store_id,
            // 作った時は窓の view の直下（`with_bounds(rect)`）・次の `frame` が箱に入れて置き直す
            applied: None,
            holes: Holes::default(),
            shown: false,
            notes: Vec::new(),
            patched: false,
        })
    }

    /// 1 frame ごと: 届いた event を判定に通し、矩形・見せる / 隠すを合わせる。`cover` = egui の popup などが上に出ている（WebView を隠す）。
    /// MIRROR-FIT-1101: `rect` = (面の矩形の全体, 見えている所)。WebView は全体の大きさで、見えている所の外を切り抜きの箱（`AppKitOp::WebClip`）が隠す。
    /// `egui_took_focus` = egui の focus がこの frame に別の widget へ移った（`focus_fix`: first responder を egui の view へ戻す・W12W-3 反証）。
    /// MIRROR-FIT-1101 追記 210: `holes` = egui が WebView の上に描いている物の矩形（`project::web::overlay_holes`）。見えている所と重なる穴は箱に抜く
    /// （`AppKitOp::WebHoles`・穴が替わった frame だけ）= 隠さない。抜けなければ（箱が無い・class が無い）重なる穴が在る間は今まで通り隠す（理由は 1 度だけ記録）。
    /// 戻り値は、この frame の後も first responder が WebView の中か（呼ぶ側が egui の focus を外す）。
    pub fn frame(&mut self, rect: Option<(Frame, Frame)>, cover: bool, holes: &[Frame], egui_took_focus: bool) -> bool {
        let now = Instant::now();
        while let Ok((gen, ev)) = self.events.try_recv() {
            self.life.on(gen, ev, now);
        }
        // wry は didStartProvisional も didFail も渡さないので、WKWebView の isLoading と estimatedProgress を毎 frame 読む（判定は WebLife・図 §18）
        let wk = self.view.webview();
        // SAFETY: 自分が作った WKWebView の読むだけの property を、AppKit の main thread（eframe の update の中）で読む
        let (loading, progress) = unsafe { (wk.isLoading(), wk.estimatedProgress()) };
        self.life.on(self.life.generation(), WebEvent::Progress { loading, progress }, now);
        self.life.tick(now);
        if self.life.want_reload {
            self.retry();
            self.notes.push("The web content process quit, so it was reloaded once".into());
        }
        let wk = &*self.view.webview() as *const _ as *mut std::ffi::c_void;
        if let Some((slot, shown)) = rect {
            if self.applied.is_none_or(|(a, b)| a.error_px(&slot) >= 0.5 || b.error_px(&shown) >= 0.5) {
                // MIRROR-FIT-1101: 箱（見えている所）を置いてから、WebView を箱から見た所に全体の大きさで置く（wry は箱の高さで裏返す）。
                // 箱を作れなければ今まで通り見えている所に縮めて置く（理由は 1 度だけ記録）
                let at = match super::appkit(AppKitOp::WebClip { view: wk, shown: Some([shown.x, shown.y, shown.w, shown.h]) }) {
                    Ok(_) => Frame { x: slot.x - shown.x, y: slot.y - shown.y, w: slot.w, h: slot.h },
                    Err(e) => {
                        let line = format!("Couldn't clip the web view, so it is shrunk to what shows: {e}");
                        if !self.notes.contains(&line) {
                            self.notes.push(line);
                        }
                        shown
                    }
                };
                let _ = self.view.set_bounds(bounds(at));
                self.applied = Some((slot, shown));
            }
        }
        if let Some(line) = self.holes.sync(wk, rect.map(|(_, s)| s), holes) {
            if !self.notes.contains(&line) {
                self.notes.push(line);
            }
        }
        let show = self.life.state == WebState::Live && !cover && rect.is_some() && !self.holes.hide;
        if show != self.shown {
            let _ = self.view.set_visible(show);
            // 箱の隠す / 見せるを WebView に揃える（隠した WebView の空の箱が egui の押しを取らない）
            let _ = super::appkit(AppKitOp::WebClip { view: wk, shown: None });
            self.shown = show;
            // CU 所見 ③: 隠した WebView（別の Page の間・popup の下）を AX の木に残さない（VoiceOver が見えない頁を読む）。見せる時に足し直す（`patched` を戻す）
            if !show && self.patched {
                let _ = super::appkit(AppKitOp::AxDropChild(wk));
                self.patched = false;
            }
        }
        if show && !self.patched {
            self.patched = true;
            for op in [AppKitOp::InstallEditMenu, AppKitOp::AxAddChild(wk)] {
                self.notes.push(super::appkit(op).unwrap_or_else(|e| format!("Failed · {e}")));
            }
        }
        let within = show && super::appkit(AppKitOp::FirstResponderWithin(wk)).is_ok_and(|v| v == "1");
        match super::focus_fix(within, egui_took_focus) {
            FocusFix::Keep => false,
            FocusFix::EguiLetsGo => true,
            // 戻せなければ前と同じく egui が手放す（欄に focus が在るように見せて key を WebView に送らない）
            FocusFix::ToHost => match super::appkit(AppKitOp::FirstResponderToHost(wk)) {
                Ok(v) if v == "1" => false,
                r => {
                    let why = r.unwrap_or_else(|e| e.to_string());
                    self.notes.push(format!("Couldn't return the first responder to the egui field: {why}"));
                    true
                }
            },
        }
    }

    /// W12W-1b: 同じ WebView で読み直す（`WebLife::retry` → handler の generation を進める → `load_url(life.url)`。`reload()` は何も読んでいない WKWebView では何もしない）。
    /// 例: `if views.state(&key) is Recoverable { surface.retry(); }`・`frame` が `life.want_reload`（1 回目の process の終了）で呼ぶ。
    /// 保証する: 戻り値は新しい generation・前の generation の event は捨てられる。保証しない: 読める事（作り直しは `PageWebViews::retry` が開けなかった面にだけする）。
    pub fn retry(&mut self) -> u64 {
        let (g, loaded) = self.load_life_url();
        if let Err(e) = loaded {
            self.notes.push(format!("Couldn't ask for a reload: {e}"));
        }
        g
    }

    /// `retry` と `load` の共通: generation を進めて `life.url` を同じ WebView で読む（YouTube は開く時と同じ Referer）。
    fn load_life_url(&mut self) -> (u64, wry::Result<()>) {
        let g = self.life.retry(Instant::now());
        self.gen.store(g, Ordering::SeqCst);
        let loaded = match referer(&self.spec) {
            Some(headers) => self.view.load_url_with_headers(&self.life.url, headers),
            None => self.view.load_url(&self.life.url),
        };
        (g, loaded)
    }

    /// 本人の browser へ渡す URL（新しい窓・sign-in はその URL、他は今の URL）
    pub fn handoff_url(&self) -> String {
        self.life.handoff_url.clone().unwrap_or_else(|| self.view.url().unwrap_or_else(|_| self.life.url.clone()))
    }

    /// WebView を隠す（handoff の後・Kangae の別の面に替えた時）
    pub fn hide(&mut self) {
        let _ = self.view.set_visible(false);
        let wk = &*self.view.webview() as *const _ as *mut std::ffi::c_void;
        let _ = super::appkit(AppKitOp::WebClip { view: wk, shown: None });
        self.shown = false;
    }

    /// W12W-B: 上の欄で URL を替える（同じ WebView・generation を進める）。
    /// 例: `surface.load("https://www.youtube.com/watch?v=x")?`
    /// 保証する: `decide_navigation` が Allow でない URL は Err の 1 行で今の頁を残す。保証しない: 頁が読める事。
    pub fn load(&mut self, url: &str) -> Result<(), String> {
        if let WebNav::Deny { why, .. } = decide_navigation(url, &self.spec.allow_hosts) {
            return Err(why);
        }
        if url == self.life.url && url == self.spec.url {
            return Ok(());
        }
        self.spec.url = url.to_owned();
        self.life.url = url.to_owned();
        let (_, loaded) = self.load_life_url();
        loaded.map_err(|e| format!("Couldn't open the page: {e}"))
    }

    /// W12W-B: 前の頁。履歴が無ければ false。
    pub fn go_back(&mut self) -> bool {
        matches!(self.view.can_go_back(), Ok(true)) && self.view.go_back().is_ok()
    }

    /// W12W-B: 次の頁。履歴が無ければ false。
    pub fn go_forward(&mut self) -> bool {
        matches!(self.view.can_go_forward(), Ok(true)) && self.view.go_forward().is_ok()
    }

    /// W12W-B: 今の URL を読み直す（`view.reload`。何も読んでいない面は `retry`）。
    pub fn reload_page(&mut self) -> Result<(), String> {
        if self.life.url.is_empty() {
            self.retry();
            return Ok(());
        }
        self.view.reload().map_err(|e| format!("Couldn't reload: {e}"))
    }

    /// W12W-2: 今の頁で JavaScript を 1 つ走らせ、結果（JSON）を `done` で返す（wry の `evaluate_script_with_callback`・main thread で呼ぶ）。
    /// Wave: W12W-2
    /// 例: `web.evaluate("JSON.stringify(performance.timeOrigin)", Box::new(move |r| { let _ = tx.send(r); }))?`
    /// 保証する: `super::may_evaluate(今の URL, &self.spec.allow_hosts)` が Err なら何も走らせず Err の 1 行（Browser の面 = allow_hosts が空は常に Err）・`done` は 1 回だけ呼ぶ（読めない答えは Err）。
    /// 保証しない: 結果が元の service の状態である事（製品の自己申告 = unverified。verified は試験の側の oracle・B2 の補助）。
    /// 最初の読み込みが終わる前の wry は script を溜めて callback を捨てる（wry-0.57.0/src/wkwebview/mod.rs:724）ので、呼ぶ側は `Live` の時だけ呼ぶ（`PageWebViews::evaluate`）。
    pub fn evaluate(&self, script: &str, done: Box<dyn Fn(Result<serde_json::Value, String>) + Send + 'static>) -> Result<(), String> {
        let url = self.view.url().map_err(|e| format!("Couldn't read the URL of this page: {e}"))?;
        super::may_evaluate(&url, &self.spec.allow_hosts)?;
        // wry は答えを JSON の文字にして渡す（undefined・例外は空の文字）
        self.view
            .evaluate_script_with_callback(script, move |raw: String| done(serde_json::from_str(&raw).map_err(|_| "Couldn't read the script's result (undefined or an error)".to_owned())))
            .map_err(|e| format!("Couldn't run the script: {e}"))
    }
}

/// MIRROR-FIT-1101 追記 210: WebView の箱に抜いた穴の今（`WebSurface` と `Canvas` の共通・統合の 12 回目で `WebSurface` から出した）。
#[derive(Default)]
struct Holes {
    /// 最後に箱に抜いた (見えている所, 穴)。替わった frame だけ `AppKitOp::WebHoles` を打つ
    holed: Option<(Frame, Vec<Frame>)>,
    /// 穴を抜けなかった（`WebHoles` が Err）ので、重なる穴が在る間は隠している
    hide: bool,
}

impl Holes {
    /// 見えている所 `shown` に重なる穴だけ箱に抜く（替わった frame だけ・無くなれば mask を外す）。抜けなければ重なる穴が在る間は `hide`（今まで通り隠す）。
    /// 戻り値 = 抜けなかった理由の 1 行（呼ぶ側が記録する）。
    fn sync(&mut self, wk: *mut std::ffi::c_void, shown: Option<Frame>, holes: &[Frame]) -> Option<String> {
        let want = shown
            .map(|s| (s, holes.iter().filter(|h| h.x < s.x + s.w && s.x < h.x + h.w && h.y < s.y + s.h && s.y < h.y + h.h).copied().collect::<Vec<_>>()))
            .filter(|(_, h)| !h.is_empty());
        if want == self.holed {
            return None;
        }
        let (s, h) = want.clone().unwrap_or_else(|| (self.holed.as_ref().map_or_else(Frame::default, |(s, _)| *s), Vec::new()));
        let op = AppKitOp::WebHoles { view: wk, shown: [s.x, s.y, s.w, s.h], holes: h.iter().map(|f| [f.x, f.y, f.w, f.h]).collect() };
        self.holed = want;
        match super::appkit(op) {
            Ok(_) => {
                self.hide = false;
                None
            }
            Err(e) => {
                self.hide = !h.is_empty();
                Some(format!("Couldn't cut holes in the web view, so it hides under what's drawn over it: {e}"))
            }
        }
    }
}

impl Drop for WebSurface {
    fn drop(&mut self) {
        // wry の Drop は WKWebView を窓から外して leak する。AX の木が孤児を指し続けないよう、自分が足した物なら外す
        let wk = &*self.view.webview() as *const _ as *mut std::ffi::c_void;
        let _ = super::appkit(AppKitOp::AxDropChild(wk));
        // MIRROR-FIT-1101: 切り抜きの箱も外す（wry の Drop は WebView を箱から外すだけで、空の箱が窓に残る）
        let _ = super::appkit(AppKitOp::WebUnclip(wk));
    }
}

/// CHAT-FREE-1101 段 2（owner 201「AI の画用紙」）: chat の ```html を描く閉じた WebView 1 つ（押した 1 つ・app.rs の `canvas_sync` が持つ）。
/// `WebSurface` と違い URL を持たない: 文書は `canvas_document`（CSP で network・file・frame を断る）・data store は使い捨て（incognito = nonPersistentDataStore）・
/// navigation は `canvas_may_navigate`（about: だけ）・新しい窓と camera / mic は deny・download handler を登録しない（wry が Cancel）・
/// IPC は `canvas_message` が `bridge` の口で通した物だけ（chat = 2 つ・AI-COMPOSE-1101 の Page の `canvas` = 3 つ）。AX の木・Edit menu・focus の手当ては足さない
/// （押して見る絵と button だけ・Drop で外す物を作らない）。
/// 新しくした理由: `WebSurface` は URL・許可の host・Page の data store・download の置き場を持つ Page の面で、閉じた文書と頁からの頼みを持たない。
pub struct Canvas {
    view: WebView,
    /// 開いた ```html の key（`ui::markdown::CanvasSlot::key`）
    pub key: u64,
    bridge: super::CanvasBridge,
    msgs: mpsc::Receiver<String>,
    applied: Option<Frame>,
    /// MIRROR-FIT-1101 追記 210（統合の 12 回目）: 箱に抜いた穴（toast・menu・palette が card の上に見える）
    holes: Holes,
    shown: bool,
    /// OPEN-INPLACE-1101: 頁へ最後に送った run の今（`run` が同じ物を送り直さない・開き直した頁は None から）
    sent: Option<super::PartRun>,
}

impl Canvas {
    /// 開く（chat は人が `Run` を押した 1 回だけ・Page の部品は見えた時・裏で作り置きしない）。開けなければ Err の 1 行。
    /// `values` = Page の部品の前に残した値（`CanvasBridge::Page` の `shibubu.values`・chat は空）。
    pub fn open<W: HasWindowHandle>(parent: &W, key: u64, html: &str, bridge: super::CanvasBridge, values: &serde_json::Map<String, serde_json::Value>, rect: Frame) -> Result<Canvas, String> {
        let (tx, rx) = mpsc::channel::<String>();
        let view = WebViewBuilder::new()
            .with_bounds(bounds(rect))
            .with_visible(false)
            .with_devtools(cfg!(debug_assertions))
            .with_incognito(true)
            .with_navigation_handler(|url| super::canvas_may_navigate(&url))
            .with_new_window_req_handler(|_url, _features| NewWindowResponse::Deny)
            .with_permission_handler(|_kind: PermissionKind| PermissionResponse::Deny)
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                let _ = tx.send(req.into_body());
            })
            .with_html(super::canvas_document(html, bridge, values))
            .build_as_child(parent)
            .map_err(|e| format!("Couldn't create the web view: {e}"))?;
        Ok(Canvas { view, key, bridge, msgs: rx, applied: None, holes: Holes::default(), shown: false, sent: None })
    }

    /// AI-COMPOSE-1101: `shibubu.read` の答えを頁へ（`canvas_answer_js`・頁の Promise が `id` で受ける）。頁が閉じていれば何もしない。
    /// `open` = 読めなかったが Shibubu が開けば読める（頁に「Let Shibubu open it」・TALK-HONEST 追記 232）。
    pub fn answer(&self, id: u64, got: &Result<serde_json::Value, String>, open: bool) {
        let _ = self.view.evaluate_script(&super::canvas_answer_js(id, got, open));
    }

    /// OPEN-INPLACE-1101 の 1: 「Let Shibubu open it」の run の今を頁の部品の所へ（`canvas_run_js`・None = 消す）。頁が閉じていれば何もしない。
    /// 毎 frame 呼んでよい: 前に頁へ送った物と同じなら script を流さない（頁を開き直した後は 1 回送り直す）。
    /// Wave: OPEN-INPLACE-1101
    pub fn run(&mut self, run: Option<&super::PartRun>) {
        if self.sent.as_ref() == run {
            return;
        }
        self.sent = run.cloned();
        let _ = self.view.evaluate_script(&super::canvas_run_js(run));
    }

    /// 1 frame ごと: 矩形を合わせて見せる / 隠す（`rect` が None = 枠の一部が見えない・別の画面・`cover` = WebView ごと隠す物）・届いた頼みを返す。
    /// MIRROR-FIT-1101 追記 210（統合の 12 回目）: `WebSurface` と同じ箱（`AppKitOp::WebClip`・見えている所 = 枠）に入れ、`holes`（`project::web::overlay_holes`）を抜く。
    /// 箱を作れなければ今まで通り枠に直に置き、重なる穴が在る間は隠す。
    pub fn frame(&mut self, rect: Option<Frame>, cover: bool, holes: &[Frame]) -> Vec<super::CanvasMsg> {
        let wk = &*self.view.webview() as *const _ as *mut std::ffi::c_void;
        if let Some(r) = rect {
            if self.applied.is_none_or(|a| a.error_px(&r) >= 0.5) {
                let at = match super::appkit(AppKitOp::WebClip { view: wk, shown: Some([r.x, r.y, r.w, r.h]) }) {
                    Ok(_) => Frame { x: 0.0, y: 0.0, w: r.w, h: r.h },
                    Err(_) => r,
                };
                let _ = self.view.set_bounds(bounds(at));
                self.applied = Some(r);
            }
        }
        // 抜けなかった理由は捨てる（Canvas は記録の欄を持たない・隠す事で分かる）
        let _ = self.holes.sync(wk, rect, holes);
        let show = rect.is_some() && !cover && !self.holes.hide;
        if show != self.shown {
            let _ = self.view.set_visible(show);
            // 箱の隠す / 見せるを WebView に揃える（隠した WebView の空の箱が egui の押しを取らない）
            let _ = super::appkit(AppKitOp::WebClip { view: wk, shown: None });
            self.shown = show;
        }
        let bridge = self.bridge;
        std::iter::from_fn(|| self.msgs.try_recv().ok()).filter_map(|m| super::canvas_message(&m, bridge)).collect()
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // MIRROR-FIT-1101（統合の 12 回目）: 箱も外す（wry の Drop は WebView を箱から外すだけで、空の箱が窓に残って押しを取る）
        let _ = super::appkit(AppKitOp::WebUnclip(&*self.view.webview() as *const _ as *mut std::ffi::c_void));
    }
}

/// TERM-XTERM-1101（owner 204 の 4「普通のterminalをshibubuで使えないの？orcaみたいに」）: Terminal 1 本の中身を描く xterm.js の WebView（`Generated::term_sync` が session ごとに持つ）。
/// 文書は `term_document`（vendor/xterm を中に持つ・外を読まない）・data store は使い捨て（incognito）・navigation は about: だけ・新しい窓と camera / mic は deny・
/// IPC は `term_message` が通した物だけ（打鍵・大きさ・link・右 click・遅れ）。`Canvas` と同じ箱（`AppKitOp::WebClip`）に入れ、`overlay_holes` の穴を抜く
/// （toast・menu・palette が Terminal の上に見えて押せる・被る間に隠さない）。Edit menu（⌘C / ⌘V）と first responder の手当ては `WebSurface` と同じ。
/// 新しくした理由: `Canvas` は AI の閉じた文書（返事と Build now の 2 つの口・押した 1 つだけ）で、出力を流し込む口・focus・theme を持たない。
pub struct TermView {
    view: WebView,
    msgs: mpsc::Receiver<String>,
    /// 頁が `TermMsg::Ready` を言った（それまで `write` は書かない）。頁を読み直したら false に戻り、次の Ready で呼ぶ側が seed を書き直す
    pub ready: bool,
    applied: Option<(Frame, Frame)>,
    holes: Holes,
    shown: bool,
    patched: bool,
    /// 頁の見た目（読み直す時の `term_document`）
    theme: super::TermTheme,
    /// WebKit の頁の process が落ちた（wry の handler が立てる・`frame` が読み直す）
    gone: Arc<std::sync::atomic::AtomicBool>,
}

impl TermView {
    /// 開く（Terminal の tab を初めて見せた frame の 1 回だけ）。`wake` = 頁から 1 通届くたびに呼ぶ（egui を起こす = 打った字を次の frame を待たずに PTY へ）。
    /// 開けなければ Err の 1 行（呼ぶ側はその session を egui の grid に戻す）。
    pub fn open<W: HasWindowHandle>(parent: &W, theme: &super::TermTheme, rect: Frame, wake: Box<dyn Fn() + Send + 'static>) -> Result<TermView, String> {
        let (tx, rx) = mpsc::channel::<String>();
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fell = gone.clone();
        let wake = Arc::new(wake);
        let woke = wake.clone();
        let view = WebViewBuilder::new()
            .with_bounds(bounds(rect))
            .with_visible(false)
            .with_devtools(cfg!(debug_assertions))
            .with_incognito(true)
            .with_navigation_handler(|url| super::canvas_may_navigate(&url))
            .with_new_window_req_handler(|_url, _features| NewWindowResponse::Deny)
            .with_permission_handler(|_kind: PermissionKind| PermissionResponse::Deny)
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                let _ = tx.send(req.into_body());
                wake();
            })
            .with_on_web_content_process_terminate_handler(move || {
                fell.store(true, Ordering::SeqCst);
                woke();
            })
            .with_html(super::term_document(theme))
            .build_as_child(parent)
            .map_err(|e| format!("Couldn't create the web view: {e}"))?;
        Ok(TermView { view, msgs: rx, ready: false, applied: None, holes: Holes::default(), shown: false, patched: false, theme: theme.clone(), gone })
    }

    /// 1 frame ごと: 矩形と穴を合わせて見せる / 隠す（`rect` = (全体, 見えている所)・None = この frame に置かれなかった〈別の tab・閉じた panel〉）・
    /// 届いた頁の言う事を返す。2 つ目 = この frame の後も first responder が頁の中か（呼ぶ側が egui の focus を外す・`WebSurface::frame` と同じ）。
    pub fn frame(&mut self, rect: Option<(Frame, Frame)>, cover: bool, holes: &[Frame], egui_took_focus: bool) -> (Vec<super::TermMsg>, bool) {
        // 頁の process が落ちた: 同じ文書で読み直す（頁が size と ready を言い直し、呼ぶ側が今の画面を書き直す）
        if self.gone.swap(false, Ordering::SeqCst) {
            self.ready = false;
            let _ = self.view.load_html(&super::term_document(&self.theme));
        }
        let wk = &*self.view.webview() as *const _ as *mut std::ffi::c_void;
        if let Some((slot, shown)) = rect {
            if self.applied.is_none_or(|(a, b)| a.error_px(&slot) >= 0.5 || b.error_px(&shown) >= 0.5) {
                // `WebSurface::frame` と同じ: 箱（見えている所）を置いてから箱の中に全体の大きさで置く・箱を作れなければ見えている所に縮めて置く
                let at = match super::appkit(AppKitOp::WebClip { view: wk, shown: Some([shown.x, shown.y, shown.w, shown.h]) }) {
                    Ok(_) => Frame { x: slot.x - shown.x, y: slot.y - shown.y, w: slot.w, h: slot.h },
                    Err(_) => shown,
                };
                let _ = self.view.set_bounds(bounds(at));
                self.applied = Some((slot, shown));
            }
        }
        // 抜けなかった理由は捨てる（隠す事で分かる・Canvas と同じ）
        let _ = self.holes.sync(wk, rect.map(|(_, s)| s), holes);
        let show = rect.is_some() && !cover && !self.holes.hide;
        if show != self.shown {
            let _ = self.view.set_visible(show);
            let _ = super::appkit(AppKitOp::WebClip { view: wk, shown: None });
            self.shown = show;
        }
        // ⌘C / ⌘V は Edit menu から first responder（頁の xterm の textarea）へ。AX の木の WebView の席は Page の Browser の物なので取らない（字は egui の terminal:<id> が持つ）
        if show && !self.patched {
            self.patched = super::appkit(AppKitOp::InstallEditMenu).is_ok();
        }
        let within = show && super::appkit(AppKitOp::FirstResponderWithin(wk)).is_ok_and(|v| v == "1");
        let focus = match super::focus_fix(within, egui_took_focus) {
            FocusFix::Keep => false,
            FocusFix::EguiLetsGo => true,
            FocusFix::ToHost => !matches!(super::appkit(AppKitOp::FirstResponderToHost(wk)).as_deref(), Ok("1")),
        };
        let msgs: Vec<super::TermMsg> = std::iter::from_fn(|| self.msgs.try_recv().ok()).filter_map(|m| super::term_message(&m)).collect();
        if msgs.contains(&super::TermMsg::Ready) {
            self.ready = true;
        }
        (msgs, focus)
    }

    /// PTY の出力を頁の xterm に書く（`term_write_script`・`ready` の前は書かずに Err）。Err = 頁がまだ無い・WebView が script を受けなかった。
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if !self.ready {
            return Err("The terminal page isn't ready yet".into());
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.view.evaluate_script(&super::term_write_script(bytes)).map_err(|e| e.to_string())
    }

    /// Dark / Light を替えた（`term_theme_script`・読み直す時の文書も替える）。
    pub fn set_theme(&mut self, theme: &super::TermTheme) {
        self.theme = theme.clone();
        let _ = self.view.evaluate_script(&super::term_theme_script(theme));
    }

    /// 頁へ focus（窓の first responder = この WebView・頁の中は `term.focus()`）。Show terminal / New terminal の後の 1 回。
    pub fn focus(&self) {
        let _ = super::appkit(AppKitOp::FocusWeb(&*self.view.webview() as *const _ as *mut std::ffi::c_void));
        let _ = self.view.evaluate_script("term.focus()");
    }
}

impl Drop for TermView {
    fn drop(&mut self) {
        // 箱も外す（`Canvas` と同じ）・AX の木に足していれば外す
        let wk = &*self.view.webview() as *const _ as *mut std::ffi::c_void;
        let _ = super::appkit(AppKitOp::AxDropChild(wk));
        let _ = super::appkit(AppKitOp::WebUnclip(wk));
    }
}

/// YouTube の埋め込みの Referer（CD-26）。開く時と読み直す時で同じ物（無いと 153 になる）。
fn referer(spec: &WebSpec) -> Option<wry::http::HeaderMap> {
    if spec.youtube.is_none() || spec.no_referer {
        return None;
    }
    let mut headers = wry::http::HeaderMap::new();
    headers.insert(wry::http::header::REFERER, wry::http::HeaderValue::from_static(KANGAE_REFERER));
    Some(headers)
}

fn bounds(r: Frame) -> Rect {
    Rect { position: LogicalPosition::new(r.x, r.y).into(), size: LogicalSize::new(r.w, r.h).into() }
}

/// RQ2（RQ-005・Delete）: WebKit の website data の store（`store_identifier` の 16 byte）を disk から消す
/// （`+[WKWebsiteDataStore removeDataStoreForIdentifier:completionHandler:]`・macOS 14）。結果は log の 1 行だけ（使っている WKWebView が在ると WebKit が断る →
/// `PageWebViews::remove_forgotten_stores` が次の起動〈`Generated::open`〉で消し直す）。macOS 13 以前・WebKit の class が無い = 何もしない。main thread から（`Generated::delete_project` と起動）。
pub fn remove_web_store(id: [u8; 16]) {
    use std::ffi::{c_char, c_void, CStr};
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    unsafe extern "C" {
        static _NSConcreteGlobalBlock: c_void;
    }
    #[repr(C)]
    struct Descriptor {
        reserved: usize,
        size: usize,
    }
    #[repr(C)]
    struct Block {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: unsafe extern "C" fn(*mut Block, *mut c_void),
        descriptor: *const Descriptor,
    }
    unsafe extern "C" fn done(_: *mut Block, error: *mut c_void) {
        eprintln!("shibubu: web: a deleted project's website data {}", if error.is_null() { "was removed" } else { "is removed at the next launch" });
    }
    static DESCRIPTOR: Descriptor = Descriptor { reserved: 0, size: std::mem::size_of::<Block>() };
    type Id = *mut c_void;
    // 理由: block2・objc2 の crate を足さずに completion handler を渡す。捕まえる物の無い global の block（BLOCK_IS_GLOBAL = 1 << 28）は Block_copy で
    // 写されず解放もされないので、leak した 1 つ（40 byte）で足りる。前提は `appkit` と同じ（引数は pointer だけ・x86_64 / arm64 とも C の呼び出し規約）
    unsafe {
        let class = objc_getClass(c"WKWebsiteDataStore".as_ptr());
        let nsuuid = objc_getClass(c"NSUUID".as_ptr());
        if class.is_null() || nsuuid.is_null() {
            return;
        }
        let msg = objc_msgSend as unsafe extern "C" fn();
        let send0: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(msg);
        let send_ptr: unsafe extern "C" fn(Id, Id, *const u8) -> Id = std::mem::transmute(msg);
        let send_bool1: unsafe extern "C" fn(Id, Id, Id) -> i8 = std::mem::transmute(msg);
        let send_remove: unsafe extern "C" fn(Id, Id, Id, *const Block) = std::mem::transmute(msg);
        let sel = |s: &CStr| sel_registerName(s.as_ptr());
        let remove = sel(c"removeDataStoreForIdentifier:completionHandler:");
        if send_bool1(class, sel(c"respondsToSelector:"), remove) == 0 {
            return;
        }
        let uuid = send_ptr(send0(nsuuid, sel(c"alloc")), sel(c"initWithUUIDBytes:"), id.as_ptr());
        if uuid.is_null() {
            return;
        }
        let block: &'static Block = Box::leak(Box::new(Block {
            isa: &raw const _NSConcreteGlobalBlock,
            flags: 1 << 28,
            reserved: 0,
            invoke: done,
            descriptor: &DESCRIPTOR,
        }));
        send_remove(class, remove, uuid, block);
        send0(uuid, sel(c"release"));
    }
}
