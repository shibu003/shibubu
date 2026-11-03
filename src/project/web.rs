//! W12W-1: Project の Page に WebView（excalidraw.com）を置く口と、WebView が落とした file を repo に保存する口。
//! 開く・矩形を合わせる・隠すのは `surface::webview::WebSurface`（feature `webview`）。ここは Page と Project の側の糊と、保存先の判定（純関数）。
//! 守る事: 新しい窓・camera / mic・IPC は今まで通り無い（`WebSurface::open`）。保存の名前は利用者が inline の欄で決める（NSSavePanel を出さない＝AX で操作できる）。
//! 読み戻し（verified）は製品ではしない: disk の file は試験の側が読む（W12W-2）。
use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
use crate::surface::{Frame, WebSpec, WebState};
use crate::ui::ax::AxTextInput;
use crate::ui::theme::{self, Variant};
use eframe::egui;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// WebView の download を受ける一時の dir の名前（Shibubu の data dir の直下）。run の初期化で data dir ごと消える。
pub const STAGING_DIR: &str = "web-downloads";
/// 保存先: Project の repo の中のこの dir（主デモの `docs/arch.excalidraw`）。無ければ作る。
pub const SAVE_DIR: &str = "docs";
/// inline の保存の欄の AX の identifier の頭（`<頭>:<surface_id>`）。試験の側はこれで欄と button を探す（`AxScope` の表には足さない＝L1 の検査の範囲の中）。
pub const SAVE_NAME_AX: &str = "web-save-name";
pub const SAVE_BUTTON_AX: &str = "web-save";
/// MIRROR-FIT-1101 追記 210（owner 210 の 2）: egui が WebView（子の NSView = egui より上）の上に描いている物の矩形 = WebView に抜く穴。
/// Order が Foreground と Tooltip と Debug の見えている Area（toast・menu・popup・palette・tooltip・inspector）と、`windows` に名指した egui の `Window`
/// （Order::Middle・承認と GitHub の取り込み）。Middle の他の Area（Web の題の帯・下の panel を被せる Area）は穴にしない（`covering` と `visible_part` の担当）。
/// 例（app.rs・CentralPanel と palette と窓を描いた後）: `let holes = web::overlay_holes(ctx, &[egui::Id::new("approval")]);` → `web.sync(frame, cover, &holes, ..)`。
/// 保証しない: 影（Area の矩形の外）・この frame に初めて出た Area の矩形（egui は前の frame の矩形を覚える = 1 frame 遅れ）。
pub fn overlay_holes(ctx: &egui::Context, windows: &[egui::Id]) -> Vec<Frame> {
    use egui::{LayerId, Order};
    let mut holes: Vec<Frame> = ctx.memory(|m| {
        let areas = m.areas();
        let named = windows.iter().map(|&id| LayerId::new(Order::Middle, id)).filter(|l| areas.is_visible(l));
        let over = areas.visible_layer_ids().into_iter().filter(|l| matches!(l.order, Order::Foreground | Order::Tooltip | Order::Debug));
        over.chain(named)
            .filter_map(|l| m.area_rect(l.id))
            .filter(|r| r.is_positive())
            .map(|r| Frame { x: r.min.x as f64, y: r.min.y as f64, w: r.width() as f64, h: r.height() as f64 })
            .collect()
    });
    // HashSet の順は frame ごとに違う → 並べる（`WebSurface::frame` は前の frame と同じ穴なら AppKit を打たない）
    holes.sort_by(|a, b| (a.y, a.x, a.h, a.w).partial_cmp(&(b.y, b.x, b.h, b.w)).unwrap_or(std::cmp::Ordering::Equal));
    holes
}

/// W12W-1b: `Recoverable` の面の矩形の中の「もう一度」の button の AX の identifier の頭（`<頭>:<surface_id>`）。押すと `PageWebViews::retry`。
/// feature `webview` の無い build（`surface::WEBVIEW_BUILT` が false）では出さない（押しても同じ理由で戻るだけ = L2）。
pub const RETRY_AX: &str = "web-retry";
/// 最初に置く Browser の URL（上の欄が空のまま足した時）。
pub const DEFAULT_BROWSER_URL: &str = "https://www.google.com/";

/// `PageWebViews` の面の key（`<project_id>/<surface_id>`）。どの Project も Diagram の面は同じ id `diagram` なので、Project を key に入れないと
/// A で書き出した file の欄が B の Page に出て B の repo に書かれる。例: `views.pending_save(&web::key(&p.id, sid))`。
pub fn key(project_id: &str, surface_id: &str) -> String {
    format!("{project_id}/{surface_id}")
}

/// RQ2: 消した Project の website data の store を次の起動まで覚える file（data dir の下・消した Project の id を 1 行ずつ。store の id は `store_identifier(Project の id, …)`）。
pub const FORGET_STORES: &str = "web-stores-to-remove";

/// key の surface_id の部分（AX の identifier `web-save:<sid>`・`web-retry:<sid>` に使う）。Project の id は `/` を持たない（`project.<app id>`）。
pub fn sid_of(key: &str) -> &str {
    key.split_once('/').map_or(key, |(_, s)| s)
}

/// G72: 面の website data の store（`surface::store_identifier` に渡す (space, software)）。`key` = `key(project, surface_id)`・`spec` = `web_spec` の結果。
/// Browser の面（`allow_hosts` が空・W12W-B・B4 の GitHub の login）は Shibubu で 1 つ（`SHARED_WEB_STORE`）。それ以外の Web の面（Diagram の excalidraw.com など
/// host の決まった面）は Project ごと（space = Project の id）: 共有の store では Excalidraw の図（localStorage）が別の Project の Diagram に出た（demo-loop の r9・r10）。
/// Wave: W12W-2
/// 例: `(spec.space_id, spec.software_id) = web::store_of(&key, &spec);`（`PageWebViews::place` が開く前に 1 回）。
/// 保証する: 同じ Project の面は再起動を越えて同じ store（Project の id は DB に残る・`project.app.<new_id>`）・別の Project は別の store・
/// Browser の面は Project に依らず `SHARED_WEB_STORE`。
/// 保証しない: Project ごとの面で login の要る頁の login（Project ごとに 1 回要る・YC_WAVES §3）・Project の store を disk から消す事（RQ2 で Project を消す道が出来た → `PageWebViews::forget_project` が消す）。
pub fn store_of(key: &str, spec: &WebSpec) -> (String, String) {
    // ponytail: Project ごとの store は disk（~/Library/WebKit/<bundle id>/WebsiteDataStore/）に残る。製品に Project を消す道が出来たら、そこで WKWebsiteDataStore.remove(forIdentifier:)（macOS 14）
    let (shared, software) = crate::surface::SHARED_WEB_STORE;
    // Browser の面 = allow_hosts が空（`web_spec` の spec.browser・`surface::may_evaluate` と同じ見分け）
    let space = if spec.allow_hosts.is_empty() { shared } else { key.split_once('/').map_or(key, |(p, _)| p) };
    (space.into(), software.into())
}

/// `sync` の帳簿（純関数）: この frame に開く面・隠す面・閉じる面。
#[derive(Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(all(feature = "webview", target_os = "macos")), allow(dead_code))]
struct SyncPlan {
    /// place されたが開いておらず、前に開けなかった事も無い（開くのは 1 回だけ・失敗は作り直さない）
    pub open: Vec<String>,
    /// 開いていて place されなかった keep_alive の面（隠す・event は読み続ける）
    pub hide: Vec<String>,
    /// 開いていて place されなかった keep_alive でない面（閉じる）
    pub close: Vec<String>,
}

#[cfg_attr(not(all(feature = "webview", target_os = "macos")), allow(dead_code))]
fn sync_plan<'a>(
    open: impl IntoIterator<Item = &'a String>,
    failed: &BTreeMap<String, String>,
    keep: &BTreeMap<String, bool>,
    placed: &[&str],
) -> SyncPlan {
    let mut plan = SyncPlan::default();
    let open: Vec<&String> = open.into_iter().collect();
    for id in &open {
        if placed.contains(&id.as_str()) {
            continue;
        }
        if keep.get(*id).copied().unwrap_or(false) {
            plan.hide.push((*id).clone());
        } else {
            plan.close.push((*id).clone());
        }
    }
    for id in placed {
        if !open.iter().any(|o| o == id) && !failed.contains_key(*id) && !plan.open.iter().any(|o| o == id) {
            plan.open.push((*id).to_owned());
        }
    }
    plan
}

/// Project の面の起こし方（`SurfaceSpec`）から WebView を開く事実（`WebSpec`）を作る。Web の面（backend = webview・target = Url）でなければ None。
/// 例: `if let Some(w) = web::web_spec(&spec, Some(data_dir.join(web::STAGING_DIR))) { views.place(&web::key(&p.id, &spec.id), w, spec.keep_alive, rect, shown) }`。
/// 保証する: `allow_hosts` は URL の host 1 つ（excalidraw.com なら json.excalidraw.com などの sub-domain も `decide_navigation` が通す）。W12W-B の Browser（`spec.browser`）は空（host の制限無し）。`download_dir` は引数のまま。
/// W12W-2: `space_id` / `software_id`（= WebKit の website data の store）は既定の `surface::SHARED_WEB_STORE`（Browser の面の物・login は 1 回・§3）。
/// G72: Browser でない面（Diagram）は `PageWebViews::place` が `store_of` で Project ごとの store に決め直す。
/// 保証しない: `blob:` の navigation（Excalidraw の書き出し）を `decide_navigation` が通すか（W12W-1 の probe 3・未測・ここでは変えない）。
/// Wave: W12W-1, W12W-2
pub fn web_spec(spec: &SurfaceSpec, download_dir: Option<PathBuf>) -> Option<WebSpec> {
    let SurfaceTarget::Url { url } = &spec.target else {
        return None;
    };
    if spec.backend != SurfaceBackendKind::WebView {
        return None;
    }
    let host = crate::surface::host_of(url)?;
    Some(WebSpec {
        url: url.clone(),
        // 既定の store（Browser の面の物）。Diagram 等は `PageWebViews::place` が `store_of` で Project ごとに替える（G72）
        software_id: crate::surface::SHARED_WEB_STORE.1.into(),
        space_id: crate::surface::SHARED_WEB_STORE.0.into(),
        // W12W-B: Browser は host の制限無し（空）。Diagram 等は今まで通り URL の host 1 つ。
        allow_hosts: if spec.browser { Vec::new() } else { vec![host] },
        download_dir,
        // G29（B3）: keep_alive の面は隠れても動き続ける物（Excalidraw が BG でも図を保存する）。Browser は任意の頁なので script を入れない（CD-25）
        report_visible: spec.keep_alive && !spec.browser,
        ..WebSpec::default()
    })
}

/// WebView が一時の dir に落とした `staged` を、Project の repo の `docs/<name>` に置く（同じ名前が在れば置き換える・同じ dir の一時 file から rename）。
/// 例: inline の欄で「保存」が押されたら `match web::save_download(&d.staged, &repo, &pending.name) { Ok(p) => toast(p), Err(e) => pending.error = Some(e.to_string()) }`。
/// 保証する: 書くのは `<workspace>/docs/` の直下だけ（`name` に `/`・`\`・`..`・NUL・先頭の `.` があれば `BadName` で書かない）・途中で落ちても前の file は壊れない。
/// 保証しない: 中身が Excalidraw の図である事・git に入る事（`git status` は試験の側と Terminal が見る）。
/// Wave: W12W-1
pub fn save_download(staged: &Path, workspace: &Path, name: &str) -> Result<PathBuf, SaveError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(SaveError::EmptyName);
    }
    if name.starts_with('.') || name.contains(['/', '\\', '\0']) || name.contains("..") {
        return Err(SaveError::BadName(name.to_owned()));
    }
    if !staged.is_file() {
        return Err(SaveError::NotStaged);
    }
    let io = |e: std::io::Error| SaveError::Io(e.to_string());
    let dir = workspace.join(SAVE_DIR);
    std::fs::create_dir_all(&dir).map_err(io)?;
    // 同じ dir の一時 file に写してから rename（途中で落ちても前の file は壊れない・staged は WebView の物なので残す）
    let tmp = dir.join(format!(".{name}.shibubu-tmp"));
    std::fs::copy(staged, &tmp).map_err(io)?;
    let to = dir.join(name);
    std::fs::rename(&tmp, &to).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })?;
    // FX-035: 書いたあとで disk を読み戻し、staged と同じ中身でなければ保存できたことにしない（`Saved` は読み戻しの後だけ）
    if !disk_matches(&to, staged) {
        return Err(SaveError::NotReadBack(name.to_owned()));
    }
    Ok(to)
}

/// 書いた file が staged と同じ中身か（読めなければ false）。
fn disk_matches(written: &Path, staged: &Path) -> bool {
    matches!((std::fs::read(written), std::fs::read(staged)), (Ok(a), Ok(b)) if a == b)
}

/// `save_download` の拒否。Display は inline の欄の下に出す 1 行で、次の一手を言う（D-e）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// 名前が空
    EmptyName,
    /// 名前に path の区切り・`..`・先頭の `.` などがある（中身 = 打たれた名前）
    BadName(String),
    /// Project に repo の場所が無い（`project::workspace_in` / `SHIBUBU_WORKSPACE_DIR` が渡されていない）
    NoWorkspace,
    /// 一時の file が無い・download がまだ終わっていない・WebKit が失敗と言った
    NotStaged,
    /// disk に書けない（中身 = OS の理由 1 行）
    Io(String),
    /// 書いたあとの読み戻しが staged と合わない（中身 = file 名）。保存できたとは言わない
    NotReadBack(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "Enter a file name"),
            Self::BadName(n) => write!(
                f,
                "“{n}” can't be a file name · no / or .."
            ),
            // PROJECT-DIR-1101: どの Project も自分の folder を持つ = 無いのは repo（「folder が無い」と言わない）
            Self::NoWorkspace => write!(
                f,
                "This project has no repo to save into · choose one with Change folder…"
            ),
            Self::NotStaged => write!(
                f,
                "The file from Excalidraw hasn't arrived"
            ),
            Self::Io(e) => write!(
                f,
                "Couldn't write to the repo ({e}) · check free space and permissions"
            ),
            Self::NotReadBack(n) => write!(f, "“{n}” was written but reading it back didn't match · save it again"),
        }
    }
}

/// 名前を待っている download 1 つ（inline の欄の中身）。`name` は WebKit が提案した file 名で始まり、利用者が書き換える。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingSave {
    pub surface_id: String,
    pub url: String,
    pub staged: PathBuf,
    pub name: String,
    /// 前の「保存」の拒否（`SaveError` の Display）
    pub error: Option<String>,
}

/// Shibubu の窓 1 つが持つ WebView の面（surface_id → 開いた `WebSurface`）。Page を移っても keep_alive の面は閉じずに隠す。
/// 使い方（1 frame）: renderer の `place_surface` → `views.place(..)`・CentralPanel の後に `views.sync(frame, cover, egui_took_focus)`・
/// `draw_page` の中で `views.pending_save(sid)` が在れば `save_row` を出し、押されたら `views.save(sid, repo)`。
/// 保証する: WebView を作るのは `place` された面だけ（人が Page を開いた結果・裏で作り置きしない）・この frame に `place` されなかった面は隠す
/// （keep_alive = false の面は閉じる）・feature `webview` の無い build では開かず、`state` が `Recoverable(WEBVIEW_MISSING)`。
/// W12W-1b: 前の `sync` で place されず、この `sync` で place された面（Page に戻った）が `Recoverable` なら `retry` する（`Live` の面は読み直さない＝図を消さない）。
/// 保証しない: 頁が使える事（`WebState::Live` は自己申告）・前面と cursor を取らない事（WKWebView を作ると app が前に出る事がある・D-g は試験の側で見る）。
/// Wave: W12W-1
pub struct PageWebViews {
    download_dir: PathBuf,
    /// (key, spec, keep_alive, 面の矩形の全体, 見えている所)
    placed: Vec<(String, WebSpec, bool, Frame, Frame)>,
    /// 前の `sync` で place された面と見えている所（`draw_page` が矩形の中に状態を描く・`slots`）
    last: Vec<(String, Frame)>,
    /// 開いた面の keep_alive（place されなかった frame に、隠すか閉じるか）
    keep: BTreeMap<String, bool>,
    #[cfg(all(feature = "webview", target_os = "macos"))]
    open: BTreeMap<String, crate::surface::webview::WebSurface>,
    failed: BTreeMap<String, String>,
    pending: BTreeMap<String, PendingSave>,
    /// W12W-B: renderer が押した「欄で Enter」と drag の終わり（`draw_page` が Host を返した後に PageIntent にする）
    go: Vec<(String, String)>,
    frames: Vec<(String, f64, f64, f64, f64)>,
}

impl PageWebViews {
    /// `download_dir` = `<data dir>/STAGING_DIR`（無ければ `sync` が作る）。
    pub fn new(download_dir: PathBuf) -> Self {
        PageWebViews {
            download_dir,
            placed: Vec::new(),
            last: Vec::new(),
            keep: BTreeMap::new(),
            #[cfg(all(feature = "webview", target_os = "macos"))]
            open: BTreeMap::new(),
            failed: BTreeMap::new(),
            pending: BTreeMap::new(),
            go: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// RQ2: 前の起動で消した Project の website data の store を消し直す（`forget_project` の ponytail）。WebView を 1 つも作る前に 1 回
    /// （`Generated::open` が data dir の `PageWebViews` を置いた直後だけ・`start` の temp の置き場では呼ばない = 試験の queue を製品が読まない）。
    /// `live` = その Project が DB に在るか（在る Project の store は消さない: 消すのに失敗した Project・同じ id で作り直した複製・反証の直し）。消した Project の id を返す。
    pub fn remove_forgotten_stores(&self, live: &dyn Fn(&str) -> bool) -> Vec<String> {
        let Some(queue) = self.forget_queue() else { return Vec::new() };
        let Ok(text) = std::fs::read_to_string(&queue) else { return Vec::new() };
        let _ = std::fs::remove_file(&queue);
        let mut gone: Vec<String> = text.lines().map(str::trim).filter(|p| !p.is_empty() && !live(p)).map(str::to_owned).collect();
        gone.dedup();
        #[cfg(all(feature = "webview", target_os = "macos"))]
        for project in &gone {
            crate::surface::webview::remove_web_store(crate::surface::store_identifier(project, crate::surface::SHARED_WEB_STORE.1));
        }
        gone
    }

    /// RQ2: その Project の store が次の起動で消える予定か（`Generated::duplicate_project` は、その id を次の起動まで使わない = 消した複製の store の中身が新しい複製に出ない）。
    pub fn queued_for_removal(&self, project_id: &str) -> bool {
        self.forget_queue().and_then(|q| std::fs::read_to_string(q).ok()).is_some_and(|t| t.lines().any(|l| l.trim() == project_id))
    }

    fn forget_queue(&self) -> Option<std::path::PathBuf> {
        self.download_dir.parent().map(|d| d.join(FORGET_STORES))
    }

    /// 前の `sync` で Page に置いた面が在るか（在る間は毎 frame の poll が要る: WKWebView の isLoading・WebLife の時計・app.rs `next_repaint`）。
    pub fn wants_frames(&self) -> bool {
        !self.last.is_empty()
    }

    /// この frame の前の `sync` で place された面と矩形（`state` と合わせて、矩形の中に読み込み中・失敗の 1 行を描く）。
    pub fn slots(&self) -> &[(String, Frame)] {
        &self.last
    }

    /// download の一時の dir（`WebSpec::download_dir` に渡す物）。
    pub fn download_dir(&self) -> &Path {
        &self.download_dir
    }

    /// この frame に Page が予約した矩形（`ActionHost::place_surface` から）。`key` = `key(project, surface_id)`・`spec` は `web_spec` の結果・`keep_alive` は `SurfaceSpec::keep_alive`。
    /// G72: website data の store は `store_of(key, spec)` で決め直す（Diagram は Project ごと・Browser は共有）。
    /// MIRROR-FIT-1101: `rect` = 面の矩形の全体（WebView の大きさ・縮めない）・`shown` = その内の見えている所（`ActionHost::place_surface`）。
    /// WebView は `rect` に置き、`shown` の外を切り抜く（下の panel が被さっても頁を縮めない）。`slots` は `shown`。
    pub fn place(&mut self, key: &str, mut spec: WebSpec, keep_alive: bool, rect: Frame, shown: Frame) {
        // 見えない矩形（scroll で外に出た・下の panel が全部被さった）は place しない = 隠す
        if shown.w < 1.0 || shown.h < 1.0 {
            return;
        }
        (spec.space_id, spec.software_id) = store_of(key, &spec);
        self.placed.retain(|(id, ..)| id != key);
        self.placed.push((key.to_owned(), spec, keep_alive, rect, shown));
    }

    /// frame の終わりに 1 回: place された面を開く・矩形を合わせる・それ以外を隠す / 閉じる・届いた download を `pending` に移す。
    /// `cover` = egui の popup などが WebView の矩形の上に出ている（隠す。IME の候補・file panel は WebKit の物なので隠さない）。
    /// `egui_took_focus` = egui の focus がこの frame に別の widget へ移った（first responder が WebView の中なら egui の view へ戻す・`surface::focus_fix`）。
    /// 戻り値 = この frame の後も first responder が WebView の中（呼ぶ側が egui の focus を外す＝ focus の枠を二重に出さない）。
    /// MIRROR-FIT-1101 追記 210: `holes` = egui が WebView の上に描いている物（`overlay_holes`）= 各 WebView に穴として抜く（隠さない）。`cover` は穴にできない物だけ
    /// （Area を持たない inline の確認など）。
    pub fn sync(&mut self, frame: &eframe::Frame, cover: bool, holes: &[Frame], egui_took_focus: bool) -> bool {
        let placed = self.take_placed();
        self.sync_open(frame, placed, cover, holes, egui_took_focus)
    }

    /// `sync` の WebView に触らない前半（試験はここを直に呼ぶ）: この frame の place を取り出し、`slots` と keep_alive を更新し、
    /// 前の `sync` で place されず今 place された面（Page に戻った）が `Recoverable` なら `retry` する（`sync_plan` の前なので、開けなかった面は同じ frame で作り直す）。
    fn take_placed(&mut self) -> Vec<(String, WebSpec, bool, Frame, Frame)> {
        let placed = std::mem::take(&mut self.placed);
        let back: Vec<String> = placed
            .iter()
            .map(|(id, ..)| id.clone())
            .filter(|id| !self.last.iter().any(|(l, _)| l == id))
            .collect();
        for id in back {
            if matches!(self.state(&id), Some(WebState::Recoverable(_))) {
                self.retry(&id);
            }
        }
        self.last = placed
            .iter()
            .map(|(id, _, _, _, shown)| (id.clone(), *shown))
            .collect();
        for (id, _, keep, ..) in &placed {
            self.keep.insert(id.clone(), *keep);
        }
        placed
    }

    #[cfg(all(feature = "webview", target_os = "macos"))]
    fn sync_open(
        &mut self,
        frame: &eframe::Frame,
        placed: Vec<(String, WebSpec, bool, Frame, Frame)>,
        cover: bool,
        holes: &[Frame],
        egui_took_focus: bool,
    ) -> bool {
        use crate::surface::webview::WebSurface;
        let ids: Vec<&str> = placed.iter().map(|(id, ..)| id.as_str()).collect();
        let plan = sync_plan(self.open.keys(), &self.failed, &self.keep, &ids);
        for id in &plan.close {
            self.open.remove(id);
        }
        for id in &plan.hide {
            if let Some(s) = self.open.get_mut(id) {
                s.frame(None, cover, &[], false);
            }
        }
        let mut focus = false;
        for (id, spec, _, rect, shown) in placed {
            if plan.open.contains(&id) {
                // 人が Page を開いた結果の 1 回だけ（裏で作り置きしない）。開けなければ理由を残して作り直さない
                match WebSurface::open(frame, spec, rect) {
                    Ok(s) => {
                        self.open.insert(id.clone(), s);
                    }
                    Err(e) => {
                        self.failed.insert(id.clone(), e);
                    }
                }
            }
            let Some(s) = self.open.get_mut(&id) else {
                continue;
            };
            focus |= s.frame(Some((rect, shown)), cover, holes, egui_took_focus);
            let mut d = s.life.download.take();
            self.absorb(&id, &mut d);
            if let Some(s) = self.open.get_mut(&id) {
                s.life.download = d;
            }
        }
        focus
    }

    /// 終わった download を名前の欄（`pending`）へ移す（`sync` が面ごとに呼ぶ）。まだ落としている物は `download` に残す・失敗は捨てる（WebLife の detail が言う）。
    /// `key` = `key(project, surface_id)`。欄の名前は wry と一時の dir が足した「 (n)」を落とした提案の名前（`surface::download_name`）。
    pub fn absorb(&mut self, key: &str, download: &mut Option<crate::surface::WebDownload>) {
        if !download.as_ref().is_some_and(|d| d.done.is_some()) {
            return;
        }
        let d = download.take().expect("今見た");
        if d.done == Some(true) {
            self.pending.insert(
                key.to_owned(),
                PendingSave {
                    surface_id: sid_of(key).to_owned(),
                    url: d.url,
                    name: crate::surface::download_name(&d.staged),
                    staged: d.staged,
                    error: None,
                },
            );
        }
    }

    #[cfg(not(all(feature = "webview", target_os = "macos")))]
    fn sync_open(
        &mut self,
        _frame: &eframe::Frame,
        placed: Vec<(String, WebSpec, bool, Frame, Frame)>,
        _cover: bool,
        _holes: &[Frame],
        _egui_took_focus: bool,
    ) -> bool {
        for (id, ..) in placed {
            self.failed
                .entry(id)
                .or_insert_with(|| crate::surface::WEBVIEW_MISSING.to_owned());
        }
        false
    }

    /// 面の今（None = まだ開いていない）。開けなかった面は `Recoverable(理由)`。
    pub fn state(&self, surface_id: &str) -> Option<WebState> {
        if let Some(why) = self.failed.get(surface_id) {
            return Some(WebState::Recoverable(why.clone()));
        }
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get(surface_id) {
            return Some(s.life.state.clone());
        }
        None
    }

    /// W12W-1b: 「もう一度」と、Recoverable の面の Page に戻った時（`sync` が呼ぶ）。`key` = `key(project, surface_id)`。
    /// 開いている面は同じ WebView で読み直す（`WebSurface::retry`）・開けなかった面は理由を消して次の `sync` で作り直す。
    /// 例: `if ui.put(b, egui::Button::new("もう一度")).clicked() { self.web.retry(&key); }`。
    /// 保証する: 戻り値 true = 読み直しか作り直しを頼んだ・false = 何もしない（`Recoverable` でない・知らない key・`WEBVIEW_BUILT` が false）。
    /// 保証しない: 読める事（また上限を過ぎれば `Recoverable`）。
    /// Wave: W12W-1b
    pub fn retry(&mut self, key: &str) -> bool {
        if !crate::surface::WEBVIEW_BUILT {
            return false;
        }
        if self.failed.remove(key).is_some() {
            return true;
        }
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get_mut(key).filter(|s| matches!(s.life.state, WebState::Recoverable(_))) {
            s.retry();
            return true;
        }
        false
    }

    /// W12W-B: 上の欄の URL を同じ WebView で開く。開いていない面は false（次の `place` が新しい spec の URL で開く）。
    pub fn load(&mut self, key: &str, url: &str) -> bool {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get_mut(key) {
            if let Err(e) = s.load(url) {
                s.notes.push(e);
                return false;
            }
            return true;
        }
        let _ = (key, url);
        false
    }

    /// W12W-B: 前の頁。開いていない・履歴が無ければ false。
    pub fn go_back(&mut self, key: &str) -> bool {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get_mut(key) {
            return s.go_back();
        }
        let _ = key;
        false
    }

    /// W12W-B: 次の頁。開いていない・履歴が無ければ false。
    pub fn go_forward(&mut self, key: &str) -> bool {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get_mut(key) {
            return s.go_forward();
        }
        let _ = key;
        false
    }

    /// W12W-B: 今の頁を読み直す。
    pub fn reload(&mut self, key: &str) -> bool {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get_mut(key) {
            return s.reload_page().is_ok();
        }
        let _ = key;
        false
    }

    /// RQ2（RQ-007）: Project を消す時に、その Project の面を閉じ、Project ごとの website data の store（`store_identifier(project_id, "web")`）を disk から消す
    /// （WKWebsiteDataStore.remove(forIdentifier:)・macOS 14）。例（`Generated::delete_project` の DB の削除が通った後）: `let _ = self.web.forget_project(project_id);`。
    /// 保証する: 共有の store（`SHARED_WEB_STORE`・Browser の login）と他の Project の store を消さない・webview の無い build は Ok で何もしない。Err = 画面の 1 行。
    /// 保証しない: Excalidraw 等の site 側に置いた物を消す事（Shibubu の外）。
    /// Wave: RQ2
    /// ponytail: wry は閉じた WKWebView を leak する（`WebSurface` の Drop）ので、今消すと WebKit が「使用中」で断る事がある。
    /// その時のために id を `<data dir>/FORGET_STORES` に残し、次の起動（WebView を作る前・`remove_forgotten_stores`）で消し直す。消せたかは log の 1 行だけ（画面に出さない）。
    pub fn forget_project(&mut self, project_id: &str) -> Result<(), String> {
        let mine = |k: &str| k.split_once('/').is_some_and(|(p, _)| p == project_id);
        self.placed.retain(|(k, ..)| !mine(k));
        self.last.retain(|(k, _)| !mine(k));
        self.keep.retain(|k, _| !mine(k));
        #[cfg(all(feature = "webview", target_os = "macos"))]
        self.open.retain(|k, _| !mine(k));
        self.failed.retain(|k, _| !mine(k));
        self.pending.retain(|k, _| !mine(k));
        self.go.retain(|(k, _)| !mine(k));
        self.frames.retain(|(k, ..)| !mine(k));
        // Project ごとの store = (Project の id, 共有の store の software)（`store_of`）。共有の store（Browser）は space が違うので触らない
        // queue には Project の id を書く（起動の時に DB と照らせる・store の id は戻せない hash）
        if let Some(queue) = self.forget_queue() {
            use std::io::Write;
            let _ = std::fs::OpenOptions::new().create(true).append(true).open(queue).and_then(|mut f| writeln!(f, "{project_id}"));
        }
        #[cfg(all(feature = "webview", target_os = "macos"))]
        crate::surface::webview::remove_web_store(crate::surface::store_identifier(project_id, crate::surface::SHARED_WEB_STORE.1));
        Ok(())
    }

    /// RQ3（RQ-028・`Restore diagram from before`）: Project ごとの store（G72）に替える前に描いた Diagram の図は、共有の store（`SHARED_WEB_STORE`）の Diagram の
    /// site（Diagram の面の `allow_hosts` の host・今は excalidraw.com）の localStorage に残っている。それを読み（共有の store で見えない WebView を 1 つ開き
    /// `evaluate`）、この Project の Diagram の面の store（`store_of`）へ書き、面を読み直す。`done` = 写した図の要素の数（0 = 前の図が無かった = 
    /// `No diagram from before was found`）か理由の 1 行（裏の thread から・呼ぶ側が egui を起こす）。
    /// 例（`Generated::restore_diagrams` の中）: `self.web.restore_diagrams(project_id, Box::new(move |r| { let _ = tx.send(r); wake(); }))?`
    /// 保証する: 前の共有の store を消さない・変えない（読むだけ）・この Project の図が空でなければ書かない（Err `This project already has a diagram`）・
    /// 他の Project の store に触らない・webview の無い build は Err `This build can't show web pages`。Err = 始められなかった 1 行（done は呼ばない）。
    /// 保証しない: Diagram の site が localStorage の形を変えた後の図（読めなければ 0 ではなく理由の 1 行）。
    /// Wave: RQ3
    pub fn restore_diagrams(&mut self, project_id: &str, done: Box<dyn Fn(Result<usize, String>) + Send + 'static>) -> Result<(), String> {
        // ponytail: RQ-028 は未済（共有の store を見えない WebView で開いて localStorage を読む口が無い・実機の測りが要る）。呼ぶ所は 0・届いたら理由の 1 行
        let _ = (project_id, done);
        Err("Restoring the diagram from before isn't available yet".into())
    }

    /// W12W-B: 開いている面の今の URL（欄の初期値・Finished の後の表示）。
    pub fn url(&self, key: &str) -> Option<String> {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get(key) {
            return Some(s.life.url.clone());
        }
        let _ = key;
        None
    }

    /// W12W-2: 開いている面（`key` = `key(project, surface_id)`）の今の頁で JavaScript を 1 つ走らせる（`WebSurface::evaluate` への唯一の道・
    /// `open` は外から見えない）。app の main thread（egui の update の中）で呼ぶ。
    /// Wave: W12W-2
    /// 例: `views.evaluate(&web::key(&p.id, sid), "JSON.stringify(performance.timeOrigin)", Box::new(move |r| { let _ = tx.send(r); }))?`
    /// 保証する: 開いていない key・`Live` でない面・feature `webview` の無い build（`surface::WEBVIEW_MISSING`）は何も走らせず Err の 1 行・
    /// 許可の判定は `WebSurface::evaluate` の中の `surface::may_evaluate` 1 か所（Browser の面 = allow_hosts が空は常に Err）・
    /// 戻り値が Err の時 `done` は呼ばない・Ok の時 1 回だけ呼ぶ（`Live` で止めるのはこのため: 最初の読み込みが終わる前の wry は script を溜めて callback を捨てる）。
    /// 保証しない: 結果が元の service の状態である事（製品の自己申告 = unverified・これで receipt を verified にしない）・script が頁を変えない事（呼ぶ側の script 次第）。
    pub fn evaluate(&self, key: &str, script: &str, done: Box<dyn Fn(Result<serde_json::Value, String>) + Send + 'static>) -> Result<(), String> {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        {
            let s = self.open.get(key).ok_or_else(|| format!("The web view for {} isn't open", sid_of(key)))?;
            if s.life.state != WebState::Live {
                return Err(format!("The web view for {} isn't ready yet", sid_of(key)));
            }
            s.evaluate(script, done)
        }
        #[cfg(not(all(feature = "webview", target_os = "macos")))]
        {
            let _ = (key, script, done);
            Err(crate::surface::WEBVIEW_MISSING.to_owned())
        }
    }

    /// W12W-B: 上の欄の Enter（`draw_page` が `take_go` して `SetSurfaceUrl`）。
    pub fn queue_go(&mut self, surface_id: String, input: String) {
        self.go.push((surface_id, input));
    }

    /// 溜めた「欄で Enter」を 1 つ取り出す。
    pub fn take_go(&mut self) -> Option<(String, String)> {
        (!self.go.is_empty()).then(|| self.go.remove(0))
    }

    /// W12W-B: 題の帯 / 端の drag の終わり。
    pub fn queue_frame(&mut self, surface_id: String, x: f64, y: f64, w: f64, h: f64) {
        self.frames.push((surface_id, x, y, w, h));
    }

    /// 溜めた frame を 1 つ取り出す。
    pub fn take_frame(&mut self) -> Option<(String, f64, f64, f64, f64)> {
        (!self.frames.is_empty()).then(|| self.frames.remove(0))
    }

    /// W12W-B: Live の頁の URL が開いた時の spec と違う面（リダイレクト・頁の中の移動）。呼ぶ側が `SetSurfaceUrl`。
    /// 同じ URL は毎 frame 書かない（出したあと `WebSpec.url` を揃える）。
    pub fn take_url_writes(&mut self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        #[cfg(all(feature = "webview", target_os = "macos"))]
        {
            use crate::surface::{persistable_url, WebState};
            for (id, s) in &mut self.open {
                let live = matches!(s.life.state, WebState::Live);
                if let Some(url) = persistable_url(&s.spec.url, &s.life.url, live) {
                    s.spec.url = url.clone();
                    out.push((id.clone(), url));
                }
            }
        }
        out
    }

    /// 面の帯に出す事実 1 行（止めた頁・download の失敗など。`WebLife::detail`）。
    pub fn detail(&self, surface_id: &str) -> Option<String> {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get(surface_id) {
            return s.life.detail.clone();
        }
        let _ = surface_id;
        None
    }

    /// 本人の browser へ渡す URL（開けなかった・handoff の面）。開いていない面は None（呼ぶ側が spec の URL を使う）。
    pub fn handoff_url(&self, surface_id: &str) -> Option<String> {
        #[cfg(all(feature = "webview", target_os = "macos"))]
        if let Some(s) = self.open.get(surface_id) {
            return Some(s.handoff_url());
        }
        let _ = surface_id;
        None
    }

    /// 名前を待っている download（在れば inline の欄を出す）。
    pub fn pending_save(&mut self, surface_id: &str) -> Option<&mut PendingSave> {
        self.pending.get_mut(surface_id)
    }

    /// 「保存」: `save_download(staged, workspace, name)`。Ok なら pending を消して保存した path、Err なら pending に理由を残して返す。
    pub fn save(
        &mut self,
        surface_id: &str,
        workspace: Option<&Path>,
    ) -> Result<PathBuf, SaveError> {
        let Some(p) = self.pending.get_mut(surface_id) else {
            return Err(SaveError::NotStaged);
        };
        let r = match workspace {
            None => Err(SaveError::NoWorkspace),
            Some(ws) => save_download(&p.staged, ws, &p.name),
        };
        match &r {
            Ok(_) => {
                let p = self.pending.remove(surface_id).expect("今見た");
                let _ = std::fs::remove_file(&p.staged); // 一時の file は repo に写したので消す
            }
            Err(e) => p.error = Some(e.to_string()),
        }
        r
    }
}

/// inline の保存の欄 1 行: 「名前」の text の欄（AX `SAVE_NAME_AX:<sid>`・AX の set value で打てる）と「repo に保存」の button（`SAVE_BUTTON_AX:<sid>`）・
/// その下に `pending.error`。戻り値 = button が押された（Enter を含む）。modal にしない。
pub fn save_row(ui: &mut egui::Ui, pending: &mut PendingSave) -> bool {
    let mut pressed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Name for the exported file in the repo's docs folder");
        let r = ui
            .add(egui::TextEdit::singleline(&mut pending.name).desired_width(240.0))
            .ax_text_input(ui, &mut pending.name);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.ctx().accesskit_node_builder(r.id, |n| {
            n.set_author_id(format!("{SAVE_NAME_AX}:{}", pending.surface_id))
        });
        let b = theme::button_compact(ui, "Save to repo", Variant::Primary);
        ui.ctx().accesskit_node_builder(b.id, |n| {
            n.set_author_id(format!("{SAVE_BUTTON_AX}:{}", pending.surface_id))
        });
        pressed = b.clicked() || enter;
    });
    if let Some(e) = &pending.error {
        crate::ui::states::inline(ui, crate::ui::states::RegionState::Error, e);
    }
    pressed
}

/// W12W-2: 開いていない面では script を走らせず、`done` も呼ばない。web_spec の store は Shibubu で 1 つ。
#[cfg(test)]
mod w12w2 {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn w12w2_page_evaluate_refuses_a_surface_that_is_not_open() {
        let views = super::PageWebViews::new(std::env::temp_dir().join("w12w2-none"));
        let called = Arc::new(AtomicBool::new(false));
        let c = called.clone();
        let r = views.evaluate(&super::key("project.p", "diagram"), "1", Box::new(move |_| c.store(true, Ordering::SeqCst)));
        let e = r.expect_err("開いていない面");
        if !crate::surface::WEBVIEW_BUILT {
            assert_eq!(e, crate::surface::WEBVIEW_MISSING);
        } else {
            assert!(e.contains("diagram") && e.contains("isn't open"), "{e}");
        }
        assert!(!called.load(Ordering::SeqCst), "Err の時 done を呼ばない");
    }

    fn spec(url: &str, browser: bool) -> crate::domain::project::SurfaceSpec {
        use crate::domain::project::{SurfaceBackendKind, SurfaceSpec, SurfaceTarget};
        SurfaceSpec {
            id: "diagram".into(),
            backend: SurfaceBackendKind::WebView,
            target: SurfaceTarget::Url { url: url.into() },
            keep_alive: true,
            frame: None,
            browser,
        }
    }

    /// G72: 開く道（`place` → `sync` → `WebSurface::open`）に渡る spec の store。Diagram（Browser でない Web の面）は Project ごと・Browser は共有・
    /// 同じ Project の再起動は同じ store。
    #[test]
    fn g72_diagram_store_is_per_project_and_browser_store_is_shared() {
        let (diagram, browser) = (super::web_spec(&spec("https://excalidraw.com/", false), None).unwrap(), super::web_spec(&spec("https://github.com/o/r", true), None).unwrap());
        let rect = crate::surface::Frame { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let store = |views: &super::PageWebViews, key: &str| {
            let (_, w, ..) = views.placed.iter().find(|(k, ..)| k == key).expect("place した面");
            crate::surface::store_identifier(&w.space_id, &w.software_id)
        };
        let shared = crate::surface::store_identifier(crate::surface::SHARED_WEB_STORE.0, crate::surface::SHARED_WEB_STORE.1);
        let (a, b) = ("project.app.18f0a-0", "project.app.18f0b-1");
        let mut views = super::PageWebViews::new(std::env::temp_dir().join("g72-store"));
        for p in [a, b] {
            views.place(&super::key(p, "diagram"), diagram.clone(), true, rect, rect);
            views.place(&super::key(p, "browser"), browser.clone(), true, rect, rect);
        }
        let (da, db) = (store(&views, &super::key(a, "diagram")), store(&views, &super::key(b, "diagram")));
        assert_ne!(da, db, "別の Project の Diagram が同じ store = A の図（localStorage）が B の Diagram に出る");
        assert!(da != shared && db != shared, "新しい Project の Diagram が共有の store = 前の run・他の Project の図から始まる（r9・r10）");
        for p in [a, b] {
            assert_eq!(store(&views, &super::key(p, "browser")), shared, "Browser の面の store が Project ごと = GitHub の login（B4）が Project ごとに要る");
        }
        // 同じ Project の再起動（新しい PageWebViews・DB に残った同じ Project の id）= 同じ store（B3: 再起動の前に描いた図が戻る）
        let mut again = super::PageWebViews::new(std::env::temp_dir().join("g72-store"));
        again.place(&super::key(a, "diagram"), diagram, true, rect, rect);
        assert_eq!(store(&again, &super::key(a, "diagram")), da, "再起動で Diagram の store が変わった = B1 の図が戻らない");
    }

    #[test]
    fn w12w2_web_spec_hosts_and_report_visible() {
        use crate::domain::project::SurfaceSpec;
        let (a, b) = (super::web_spec(&spec("https://excalidraw.com/", false), None).unwrap(), super::web_spec(&spec("https://github.com/o/r", true), None).unwrap());
        assert_eq!(a.allow_hosts, vec!["excalidraw.com".to_string()]);
        assert!(b.allow_hosts.is_empty());
        // G29（B3）: 隠れても見えていると答えるのは keep_alive の Diagram だけ。Browser（任意の頁）と keep_alive でない面には script を入れない
        assert!(a.report_visible, "Diagram が BG で hidden のまま = Excalidraw が図を保存せず、再起動で消える");
        assert!(!b.report_visible, "Browser の任意の頁に script を入れた（CD-25）");
        let gone = super::web_spec(&SurfaceSpec { keep_alive: false, ..spec("https://excalidraw.com/", false) }, None).unwrap();
        assert!(!gone.report_visible, "閉じる面は隠れて動き続けない");
        assert!(crate::surface::REPORT_VISIBLE_JS.contains(r#""hidden""#) && crate::surface::REPORT_VISIBLE_JS.contains(r#""visible""#));
    }

    /// RQ2 反証の直し: 消した Project の store の queue は Project の id で持ち、次の起動では DB に在る Project（消すのに失敗した・同じ id で作り直した）を飛ばす。
    /// 負の対照（commit message）: `remove_forgotten_stores` の `!live(p)` を外す → 生きている Project も消す側に入って赤。
    #[test]
    fn rq2_forgotten_store_queue_skips_projects_that_still_exist() {
        let dir = std::env::temp_dir().join(format!("rq2-forget-{}", crate::storage::new_id()));
        std::fs::create_dir_all(&dir).unwrap(); // data dir（製品は起動の時に在る）
        let mut views = super::PageWebViews::new(dir.join("staging"));
        let (gone, kept) = ("project.app.gone", "project.app.kept-copy");
        views.forget_project(gone).unwrap();
        views.forget_project(kept).unwrap();
        assert!(views.queued_for_removal(gone) && views.queued_for_removal(kept) && !views.queued_for_removal("project.app.other"));
        assert_eq!(views.remove_forgotten_stores(&|p| p == kept), vec![gone.to_string()], "DB に在る Project の store を消す側に入れた");
        assert!(!views.queued_for_removal(gone), "読んだ queue が残った（毎回の起動で消し直す）");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    /// FX6（FX-035・UIUX W04）: 保存は書いたあとの読み戻しが合った時だけ成功（合わない・読めない file を `Saved` にしない）。
    /// 負の対照（commit message）: `disk_matches` を常に true にする → 赤。
    #[test]
    fn fx6_save_counts_only_when_the_disk_reads_back_the_same() {
        let dir = std::env::temp_dir().join(format!("shibubu-fx6-readback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        std::fs::write(&a, b"same").unwrap();
        std::fs::write(&b, b"same").unwrap();
        std::fs::write(&c, b"different").unwrap();
        assert!(disk_matches(&a, &b));
        assert!(!disk_matches(&a, &c), "違う中身を一致と言った");
        assert!(!disk_matches(&a, &dir.join("missing")), "読めない file を一致と言った");
        let ws = dir.join("ws");
        let p = save_download(&a, &ws, "d.excalidraw").expect("同じ中身は保存できる");
        assert_eq!(std::fs::read(p).unwrap(), b"same");
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;
    use crate::domain::project::{SurfaceBackendKind, SurfaceTarget};

    /// MIRROR-FIT-1101 追記 210: 穴 = Foreground の Area（toast・menu）と名指した Window・Middle の他の Area（Web の題の帯）は穴にしない。
    /// 負の対照: Order の絞りを外す → 帯の Area が穴に入って赤・`windows` を見ない → 承認の窓が無く赤。
    #[test]
    fn mirror_fit_210_holes_are_what_egui_draws_over_the_web_view() {
        let ctx = egui::Context::default();
        let mut holes = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::Area::new(egui::Id::new("toast")).order(egui::Order::Foreground).fixed_pos(egui::pos2(900.0, 60.0)).show(ctx, |ui| ui.allocate_space(egui::vec2(280.0, 64.0)));
                egui::Area::new(egui::Id::new("web-band")).order(egui::Order::Middle).fixed_pos(egui::pos2(300.0, 100.0)).show(ctx, |ui| ui.allocate_space(egui::vec2(400.0, 28.0)));
                egui::Window::new("Needs approval").id(egui::Id::new("approval")).fixed_pos(egui::pos2(400.0, 300.0)).show(ctx, |ui| ui.label("Approve?"));
                holes = overlay_holes(ctx, &[egui::Id::new("approval")]);
            });
        }
        let has = |x: f64, y: f64| holes.iter().any(|h| x >= h.x && x < h.x + h.w && y >= h.y && y < h.y + h.h);
        assert!(has(1000.0, 80.0), "toast が穴に無い: {holes:?}");
        assert!(has(420.0, 320.0), "承認の窓が穴に無い: {holes:?}");
        assert!(!has(500.0, 110.0), "Web の題の帯（Middle の Area）を穴にした: {holes:?}");
    }

    /// W2b-AX と合わせる: Page に面を置いている間だけ poll の frame を頼む（置いていない時は idle の frame 0 のまま）
    #[test]
    fn wants_frames_only_while_a_surface_is_on_the_page() {
        let mut v = PageWebViews::new(std::env::temp_dir());
        assert!(!v.wants_frames(), "面の無い Page で frame を頼む");
        v.last.push(("diagram".into(), Frame { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }));
        assert!(v.wants_frames(), "置いた WebView の面を poll しない（読み込みの状態と時計が進まない）");
    }

    fn diagram() -> SurfaceSpec {
        SurfaceSpec {
            id: "diagram".into(),
            backend: SurfaceBackendKind::WebView,
            target: SurfaceTarget::Url {
                url: "https://excalidraw.com".into(),
            },
            keep_alive: true,
            frame: None,
            browser: false,
        }
    }

    /// W12W-1 の契約（呼ぶ側の形）: Diagram の面は WebView・host は excalidraw.com だけ・download の dir はそのまま。App の面は None。
    #[test]
    fn w12w1_web_spec_for_the_diagram_surface() {
        let dl = PathBuf::from("/tmp/shibubu-data/web-downloads");
        let w = web_spec(&diagram(), Some(dl.clone())).expect("Diagram は WebView");
        assert_eq!(
            (
                w.url.as_str(),
                w.allow_hosts.as_slice(),
                w.download_dir.as_deref()
            ),
            (
                "https://excalidraw.com",
                &["excalidraw.com".to_string()][..],
                Some(dl.as_path())
            )
        );
        let docs = SurfaceSpec {
            backend: SurfaceBackendKind::NativeDirect,
            target: SurfaceTarget::App {
                bundle_id: "com.apple.TextEdit".into(),
                file: None,
            },
            ..diagram()
        };
        assert!(
            web_spec(&docs, None).is_none(),
            "App の面は WebView にしない"
        );
        let mut browser = diagram();
        browser.browser = true;
        browser.target = SurfaceTarget::Url { url: "https://www.google.com/".into() };
        let b = web_spec(&browser, None).expect("Browser は WebView");
        assert!(b.allow_hosts.is_empty(), "Browser は host の制限無し: {:?}", b.allow_hosts);
        assert_eq!(b.url, "https://www.google.com/");
    }

    /// W12W-1 の契約: 保存は `<repo>/docs/<name>` だけ・置き換えられる・repo の外を指す名前は書かない。
    #[test]
    fn w12w1_save_download_writes_only_under_docs() {
        let root = std::env::temp_dir().join(format!("w12w1-save-{}", std::process::id()));
        let (repo, staged) = (root.join("repo"), root.join("staged.excalidraw"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(&staged, "{\"type\":\"excalidraw\",\"n\":1}").unwrap();
        let p = save_download(&staged, &repo, "arch.excalidraw").expect("docs/ に置ける");
        assert_eq!(p, repo.join("docs/arch.excalidraw"));
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "{\"type\":\"excalidraw\",\"n\":1}"
        );
        std::fs::write(&staged, "{\"type\":\"excalidraw\",\"n\":2}").unwrap();
        save_download(&staged, &repo, "arch.excalidraw")
            .expect("同じ名前は置き換える（W12W-2 の B3）");
        assert!(std::fs::read_to_string(&p).unwrap().contains("\"n\":2"));
        for bad in [
            "../x.excalidraw",
            "a/b.excalidraw",
            ".git",
            "..",
            "a\\b",
            "x..excalidraw",
            "a\0b",
        ] {
            assert!(
                matches!(
                    save_download(&staged, &repo, bad),
                    Err(SaveError::BadName(_))
                ),
                "{bad}"
            );
        }
        assert_eq!(
            save_download(&staged, &repo, "  "),
            Err(SaveError::EmptyName)
        );
        assert_eq!(
            save_download(&root.join("missing"), &repo, "x.excalidraw"),
            Err(SaveError::NotStaged)
        );
        assert!(!root.join("x.excalidraw").exists() && !repo.join("x.excalidraw").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// W12W-1 の契約: 一時の path は dir の直下（提案の名前が `..` や絶対 path でも外に出ない）・同じ名前は重ねない。
    #[test]
    fn w12w1_staged_path_stays_in_the_dir() {
        let dir = std::env::temp_dir().join(format!("w12w1-stage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for s in [
            "/Users/x/Downloads/Untitled.excalidraw",
            "../../etc/passwd",
            "",
            "Untitled.excalidraw",
        ] {
            let p = crate::surface::staged_download_path(&dir, Path::new(s));
            assert_eq!(p.parent(), Some(dir.as_path()), "{s} → {}", p.display());
        }
        std::fs::write(dir.join("Untitled.excalidraw"), "a").unwrap();
        assert_ne!(
            crate::surface::staged_download_path(&dir, Path::new("Untitled.excalidraw")),
            dir.join("Untitled.excalidraw")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W12W-1: WebView の download の状態機械（started → completed）。別の URL の終わり・取り出した後の終わりで pending を作らない。失敗は帯の 1 行。
    #[test]
    fn w12w1_web_life_tracks_one_download() {
        use crate::surface::{WebDownload, WebEvent, WebLife};
        let now = std::time::Instant::now();
        let mut life = WebLife::new("https://excalidraw.com", false, now);
        let staged = PathBuf::from("/tmp/dl/Untitled.excalidraw");
        let url = "blob:https://excalidraw.com/1".to_string();
        life.on(
            0,
            WebEvent::DownloadStaged {
                url: url.clone(),
                staged: staged.clone(),
            },
            now,
        );
        life.on(
            0,
            WebEvent::DownloadDone {
                url: "blob:https://excalidraw.com/other".into(),
                ok: true,
            },
            now,
        );
        assert_eq!(
            life.download.as_ref().map(|d| d.done),
            Some(None),
            "別の URL の終わりで埋めない"
        );
        life.on(
            0,
            WebEvent::DownloadDone {
                url: url.clone(),
                ok: true,
            },
            now,
        );
        assert_eq!(
            life.download,
            Some(WebDownload {
                url: url.clone(),
                staged,
                done: Some(true)
            })
        );
        life.download = None;
        life.on(
            0,
            WebEvent::DownloadDone {
                url: url.clone(),
                ok: false,
            },
            now,
        );
        assert!(
            life.download.is_none(),
            "取り出した後の終わりで pending を作らない"
        );
        assert!(
            life.detail.as_deref().is_some_and(|d| d.contains("failed")),
            "失敗は帯に言う: {:?}",
            life.detail
        );
    }

    /// W12W-1: 「repo に保存」: repo が無ければ pending を残して理由を欄に出す・通れば pending と一時の file を消す。
    #[test]
    fn w12w1_page_save_keeps_the_row_until_it_lands() {
        let root = std::env::temp_dir().join(format!("w12w1-page-{}", std::process::id()));
        let (repo, dl) = (root.join("repo"), root.join(STAGING_DIR));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&dl).unwrap();
        let staged = dl.join("Untitled.excalidraw");
        std::fs::write(&staged, "{}").unwrap();
        let mut views = PageWebViews::new(dl.clone());
        assert_eq!(
            views.save("diagram", Some(&repo)),
            Err(SaveError::NotStaged),
            "名前を待つ download が無い"
        );
        views.pending.insert(
            "diagram".into(),
            PendingSave {
                surface_id: "diagram".into(),
                url: "blob:x".into(),
                staged: staged.clone(),
                name: "arch.excalidraw".into(),
                error: None,
            },
        );
        assert_eq!(views.save("diagram", None), Err(SaveError::NoWorkspace));
        let row = views.pending_save("diagram").expect("拒否の後も欄は残る");
        assert!(
            row.error.as_deref().is_some_and(|e| e.contains("folder")),
            "{:?}",
            row.error
        );
        assert_eq!(
            views.save("diagram", Some(&repo)),
            Ok(repo.join("docs/arch.excalidraw"))
        );
        assert!(views.pending_save("diagram").is_none() && !staged.exists());
        assert_eq!(
            std::fs::read_to_string(repo.join("docs/arch.excalidraw")).unwrap(),
            "{}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// W12W-1 反証の直し: sync の帳簿。place された面は開く（1 回だけ・前に開けなかった面は作り直さない）・place されなかった keep_alive の面は隠す・
    /// それ以外は閉じる。負の対照（commit message）: keep_alive の判定を外すと隠す面が閉じる面に入って赤。
    #[test]
    fn w12w1_sync_plan_opens_once_hides_keep_alive_and_closes_the_rest() {
        let open = ["a".to_string(), "b".to_string(), "c".to_string()];
        let failed = BTreeMap::from([("d".to_string(), "WebView を作れません".to_string())]);
        let keep = BTreeMap::from([("a".to_string(), true), ("b".to_string(), false), ("c".to_string(), true)]);
        let plan = sync_plan(open.iter(), &failed, &keep, &["c", "d", "e", "e"]);
        assert_eq!(
            plan,
            SyncPlan {
                open: vec!["e".into()],
                hide: vec!["a".into()],
                close: vec!["b".into()],
            }
        );
        assert_eq!(sync_plan(open.iter(), &failed, &BTreeMap::new(), &[]).close.len(), 3, "keep_alive の記録が無い面は閉じる");
    }

    /// W12W-1b: 開けなかった面（Recoverable）は、Page に戻った時（前の sync で place されていない）と「もう一度」で作り直しを頼む。
    /// 同じ Page に居続ける面は毎 frame 作り直さない。WebView の無い build では何もしない（押しても同じ理由で戻るだけ = L2）。
    /// 負の対照（commit message）: 「前の sync で place されていない」の filter を外すと、feature webview の build で 2 つ目が赤。
    #[test]
    fn w12w1b_retry_on_return_and_the_retry_button_rebuild_only_where_webview_exists() {
        use crate::surface::WEBVIEW_BUILT;
        let mut v = PageWebViews::new(PathBuf::from("/tmp/w12w1b-dl"));
        let k = key("project.app.a", "diagram");
        let r = Frame { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let spec = || web_spec(&diagram(), None).expect("Diagram は WebView");
        assert!(!v.retry(&k), "知らない key は何もしない");
        v.failed.insert(k.clone(), "WebView を作れません: x".into());
        v.place(&k, spec(), true, r, r);
        assert_eq!(v.take_placed().len(), 1);
        assert_eq!(v.state(&k).is_none(), WEBVIEW_BUILT, "Page に戻った Recoverable の面: WebView の在る build だけ理由を消して、同じ frame の sync_plan で作り直す");
        v.failed.insert(k.clone(), "WebView を作れません: y".into());
        v.place(&k, spec(), true, r, r);
        v.take_placed();
        assert!(matches!(v.state(&k), Some(WebState::Recoverable(_))), "同じ Page に居続ける面は自分では作り直さない");
        assert_eq!(v.retry(&k), WEBVIEW_BUILT, "「もう一度」");
        assert_eq!(v.state(&k).is_none(), WEBVIEW_BUILT);
    }

    /// W12W-1 反証の直し: 終わった download だけを欄へ移す（key の Project ごと）・欄の名前は wry と一時の dir の「 (n)」を落とした提案の名前・
    /// AX の identifier に使う surface_id は key の面の部分。まだ落としている物は残し、失敗は捨てる。
    #[test]
    fn w12w1_absorb_moves_only_finished_downloads_with_the_suggested_name() {
        use crate::surface::WebDownload;
        let mut views = PageWebViews::new(PathBuf::from("/tmp/dl"));
        let k = key("project.app.a", "diagram");
        let dl = |done| Some(WebDownload { url: "blob:x".into(), staged: PathBuf::from("/tmp/dl/Untitled (1).excalidraw"), done });
        let mut going = dl(None);
        views.absorb(&k, &mut going);
        assert!(going.is_some() && views.pending_save(&k).is_none(), "落としている間は欄を出さない");
        let mut failed = dl(Some(false));
        views.absorb(&k, &mut failed);
        assert!(failed.is_none() && views.pending_save(&k).is_none(), "失敗は欄にしない");
        views.absorb(&k, &mut dl(Some(true)));
        let p = views.pending_save(&k).expect("終わった download は欄へ");
        assert_eq!((p.surface_id.as_str(), p.name.as_str()), ("diagram", "Untitled.excalidraw"));
        assert!(views.pending_save(&key("project.app.b", "diagram")).is_none(), "別の Project の key には出ない");
    }

    /// W12W-1 反証の直し: wry は提案の名前を ~/Downloads と比べて「 (n)」を足してから download_started を呼ぶ（wry-0.57.0/src/wkwebview/download.rs:57-72）。
    /// repo の名前の既定はそれを落とした名前・一時の dir の番号は dir の中だけで付く。負の対照（commit message）: 落とす行を外すと赤。
    #[test]
    fn w12w1_download_name_drops_the_downloads_folder_number() {
        use crate::surface::{download_name, staged_download_path};
        for (s, want) in [
            ("/Users/x/Downloads/Untitled (1).excalidraw", "Untitled.excalidraw"),
            ("/Users/x/Downloads/Untitled (12).excalidraw", "Untitled.excalidraw"),
            ("/Users/x/Downloads/a (b).excalidraw", "a (b).excalidraw"),
            ("/Users/x/Downloads/ (1).excalidraw", " (1).excalidraw"),
            ("/Users/x/Downloads/plan (3)", "plan"),
            ("/Users/x/Downloads/.hidden (1).txt", "hidden.txt"),
            ("", "download"),
        ] {
            assert_eq!(download_name(Path::new(s)), want, "{s}");
        }
        let dir = std::env::temp_dir().join(format!("w12w1-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(staged_download_path(&dir, Path::new("/Users/x/Downloads/Untitled (1).excalidraw")), dir.join("Untitled.excalidraw"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W12W-B: 欄の Enter と drag の終わりは Host が溜めて draw_page が 1 つずつ PageIntent にする。負の対照: take を外すと 2 つ目が残ったまま赤。
    #[test]
    fn w12wb_queue_go_and_frame_are_taken_one_at_a_time() {
        let mut v = PageWebViews::new(PathBuf::from("/tmp/w12wb"));
        v.queue_go("b1".into(), "example.com".into());
        v.queue_go("b1".into(), "猫".into());
        v.queue_frame("b1".into(), 8.0, 16.0, 400.0, 300.0);
        assert_eq!(v.take_go(), Some(("b1".into(), "example.com".into())));
        assert_eq!(v.take_go(), Some(("b1".into(), "猫".into())));
        assert_eq!(v.take_go(), None, "取り出した後は空");
        assert_eq!(v.take_frame(), Some(("b1".into(), 8.0, 16.0, 400.0, 300.0)));
        assert_eq!(v.take_frame(), None);
        assert!(v.take_url_writes().is_empty(), "開いていない面は URL を書かない");
        assert!(!v.go_back("missing") && !v.go_forward("missing") && !v.reload("missing") && !v.load("missing", "https://example.com/"));
    }
}
