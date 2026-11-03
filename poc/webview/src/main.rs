//! v0.13 M4 の最初の一手: eframe（egui）の窓の中に WKWebView（wry）を child で置けるかの実測 PoC。
//! 本体の crate とは別 package（root の workspace に入れない）。測った結果は docs/internal/poc-webview.md。
//!
//! ```sh
//! cargo run -- standalone [URL] [--log PATH]                    # 項目 1: 単独の窓（winit）に WebView
//! cargo run -- child [--no-webview] [--autoplay] [--store NAME] [--log PATH]   # 項目 2〜: eframe の窓の右側に child の WebView
//! swift scripts/input-source.swift [ID | --list]                # IME 試験の入力ソース切替（引数なしで今の ID）
//! ```
//!
//! 頁の中の出来事（drag・file・IME の composition・YouTube の状態）は JS → `window.ipc.postMessage` → log に原文で出す。
//! 左の操作 panel は egui の button（AccessKit で AX に出る）なので、cua-driver の `click`（element_token）で押せる。

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::{class, msg_send, sel};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::{header::REFERER, HeaderMap, HeaderValue, Request, Response};
use wry::{DragDropEvent, NewWindowFeatures, NewWindowResponse, PageLoadEvent, Rect, WebView, WebViewBuilder, WebViewBuilderExtDarwin, WebViewExtDarwin};

// ---- log: stdout と --log の file に同じ行。左の panel にも直近 12 行 ----

static START: OnceLock<Instant> = OnceLock::new();
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
static RECENT: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

fn log(line: impl AsRef<str>) {
    let line = format!("[{:8.3}] {}", START.get_or_init(Instant::now).elapsed().as_secs_f64(), line.as_ref());
    println!("{line}");
    if let Some(f) = LOG_FILE.lock().unwrap().as_mut() { let _ = writeln!(f, "{line}"); }
    let mut recent = RECENT.lock().unwrap();
    recent.push_back(line.chars().take(200).collect());
    while recent.len() > 12 { recent.pop_front(); }
}

// ---- 試験頁: 127.0.0.1 の小さな HTTP と custom protocol（kpoc://localhost/）の両方から同じ物を配る ----
// with_html は origin が null になる（wry の doc）。YouTube の埋め込みは origin で振る舞いが変わるので、origin の有る 2 経路で測る。

const HEAD: &str = r#"<!doctype html><html lang="ja"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<style>body{font:16px -apple-system,"Hiragino Sans",sans-serif;margin:24px;color:#1d1d1f}pre{font:12px ui-monospace,monospace;white-space:pre-wrap;background:#f4f4f6;padding:8px;max-height:240px;overflow:auto}button,input,textarea{font:inherit}</style>
<script>
function send(kind,data){var line=kind+' '+JSON.stringify(data||{});var o=document.getElementById('out');if(o)o.textContent=line+'\n'+o.textContent;
try{window.ipc.postMessage(JSON.stringify({page:location.pathname,kind:kind,data:data||{}}))}catch(e){}}
window.addEventListener('error',function(e){send('js-error',{message:e.message,src:e.filename,line:e.lineno})});
</script>
"#;

const INDEX: &str = r#"<h1>Kangae PoC — WebView の試験頁</h1>
<ul><li><a href="/dnd.html">drag &amp; drop</a><li><a href="/file.html">input type=file</a><li><a href="/ime.html">IME</a><li><a href="/yt.html">YouTube IFrame Player API</a></ul>
<pre id="out"></pre><script>send('ready',{origin:location.origin})</script>"#;

const DND: &str = r#"<h1>HTML drag &amp; drop</h1>
<div id="src" draggable="true" style="width:240px;padding:16px;background:#d8e6f8;border-radius:8px;cursor:grab">これを掴んで下の枠へ</div>
<div id="dst" style="margin-top:20px;height:150px;border:3px dashed #8a8a8e;border-radius:8px;display:flex;align-items:center;justify-content:center">ここへ drop（Finder の file も可）</div>
<pre id="out"></pre>
<script>
var src=document.getElementById('src'),dst=document.getElementById('dst');
src.addEventListener('dragstart',function(e){e.dataTransfer.setData('text/plain','kangae-poc');send('dragstart')});
dst.addEventListener('dragenter',function(e){e.preventDefault();send('dragenter',{types:Array.from(e.dataTransfer.types)})});
dst.addEventListener('dragover',function(e){e.preventDefault()});
dst.addEventListener('drop',function(e){e.preventDefault();send('drop',{text:e.dataTransfer.getData('text/plain'),
files:Array.from(e.dataTransfer.files).map(function(f){return{name:f.name,size:f.size}})})});
send('ready',{origin:location.origin});
</script>"#;

const FILE: &str = r#"<h1>&lt;input type=file&gt;</h1>
<p><input id="f" type="file" multiple></p>
<pre id="out"></pre>
<script>
var f=document.getElementById('f');
f.addEventListener('click',function(){send('click')});
f.addEventListener('change',function(){send('change',{files:Array.from(f.files).map(function(x){return{name:x.name,size:x.size}})})});
f.addEventListener('cancel',function(){send('cancel')});
send('ready',{origin:location.origin});
</script>"#;

const IME: &str = r#"<h1>IME（日本語入力）</h1>
<p><input id="ime" placeholder="ここに日本語を入力" style="font-size:22px;width:90%"></p>
<p><textarea id="ta" rows="3" style="font-size:22px;width:90%" placeholder="textarea"></textarea></p>
<pre id="out"></pre>
<script>
['ime','ta'].forEach(function(id){var el=document.getElementById(id);
['compositionstart','compositionupdate','compositionend'].forEach(function(t){el.addEventListener(t,function(e){send(t,{id:id,data:e.data})})});
el.addEventListener('input',function(e){send('input',{id:id,value:el.value,isComposing:e.isComposing,inputType:e.inputType})});
el.addEventListener('keydown',function(e){send('keydown',{id:id,key:e.key,meta:e.metaKey,isComposing:e.isComposing,keyCode:e.keyCode})});
el.addEventListener('focus',function(){send('focus',{id:id})});
el.addEventListener('blur',function(){send('blur',{id:id})});});
send('ready',{origin:location.origin});
</script>"#;

// 公式の IFrame Player API（https://developers.google.com/youtube/iframe_api_reference）。動画は同 doc の例と同じ M7lc1UVf-VE。
const YT: &str = r#"<h1>YouTube IFrame Player API</h1>
<div id="player"></div>
<p><button id="play">▶ 再生</button> <button id="pause">❚❚ 一時停止</button></p>
<pre id="out"></pre>
<script>
var player=null;
var tag=document.createElement('script');tag.src='https://www.youtube.com/iframe_api';
tag.onerror=function(){send('iframe_api-load-error')};document.head.appendChild(tag);
function onYouTubeIframeAPIReady(){send('api-ready');
player=new YT.Player('player',{height:'270',width:'480',videoId:'M7lc1UVf-VE',playerVars:{playsinline:1},
events:{onReady:function(){send('onReady',{muted:player.isMuted(),volume:player.getVolume()})},
onStateChange:function(e){send('onStateChange',{state:e.data,t:player.getCurrentTime()})},
onError:function(e){send('onError',{code:e.data})},
onAutoplayBlocked:function(){send('onAutoplayBlocked')}}});
setTimeout(function(){var i=document.querySelector('iframe');send('iframe-src',{src:i&&i.src})},500);}
document.getElementById('play').onclick=function(){send('play-click');if(player)player.playVideo()};
document.getElementById('pause').onclick=function(){if(player)player.pauseVideo()};
setInterval(function(){if(player&&player.getPlayerState)send('tick',{state:player.getPlayerState(),
t:Math.round(player.getCurrentTime()*10)/10,muted:player.isMuted(),volume:player.getVolume()})},3000);
send('ready',{origin:location.origin,href:location.href});
</script>"#;

// CD-25 (1): data store を (space × software) で分ける。kpoc://localhost は起動ごとに port が変わらない固定の origin
const STORE: &str = r#"<h1>data store（localStorage）</h1><pre id="out"></pre>
<script>var k='kangae-poc-store';var before=localStorage.getItem(k);
if(!before)localStorage.setItem(k,'set '+new Date().toISOString());
send('store',{origin:location.origin,before:before,after:localStorage.getItem(k)});</script>"#;

// CD-25 (4): Kangae の custom scheme の頁に別 origin の iframe を置き、iframe から native の handler を直に呼ぶ
const IPC_TOP: &str = r#"<h1>IPC: 別 origin の iframe から</h1>
<iframe src="{BASE}/ipc-frame.html" width="640" height="140"></iframe><pre id="out"></pre>
<script>send('ready',{origin:location.origin,frame:'top'})</script>"#;

const IPC_FRAME: &str = r#"<p>iframe（別 origin）</p><pre id="out2"></pre><script>
var r={origin:location.origin,shim:typeof window.ipc,handler:!!(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.ipc)};
try{window.webkit.messageHandlers.ipc.postMessage(JSON.stringify({page:'ipc-frame',kind:'from-iframe',data:r}));r.posted=true}catch(e){r.error=String(e)}
document.getElementById('out2').textContent=JSON.stringify(r);</script>"#;

// CD-26: top-level の /embed/ に Referer header を付けて読む（PoC-A）。with_html（baseURL None）の iframe が負の対照
const YT_EMBED: &str = "https://www.youtube.com/embed/M7lc1UVf-VE?autoplay=1&mute=1&playsinline=1";
const KANGAE_REFERER: &str = "https://app.kangae.desktop"; // build-mac.sh の bundle id

fn yt_html() -> String {
    format!(r#"<!doctype html><meta charset="utf-8"><body style="margin:24px;font:16px -apple-system,sans-serif"><h1>with_html（baseURL None）の iframe</h1>
<iframe width="480" height="270" src="{YT_EMBED}" allow="autoplay; encrypted-media" allowfullscreen></iframe></body>"#)
}

const WEBGL_PROBE: &str = "(function(){var c=document.createElement('canvas');var g=c.getContext('webgl2')||c.getContext('webgl');\
if(!g)return JSON.stringify({webgl:false});var d=g.getExtension('WEBGL_debug_renderer_info');\
return JSON.stringify({webgl:true,version:g.getParameter(g.VERSION),renderer:d?g.getParameter(d.UNMASKED_RENDERER_WEBGL):g.getParameter(g.RENDERER),\
text:(document.body?document.body.innerText:'').replace(/\\s+/g,' ').slice(0,160)})})()";

const FIELD_PROBE: &str = "(function(){var e=document.getElementById('ime');var a=document.activeElement;\
return JSON.stringify({value:e&&e.value,start:e&&e.selectionStart,end:e&&e.selectionEnd,active:a&&(a.id||a.tagName),hasFocus:document.hasFocus()})})()";

const PAGE_PROBE: &str ="JSON.stringify({href:location.href,title:document.title,ua:navigator.userAgent,\
email_input:!!document.querySelector('input[type=email]'),text:(document.body?document.body.innerText:'').replace(/\\s+/g,' ').slice(0,300)})";

fn page(path: &str, base: &str) -> Option<String> {
    let body = match path.split('?').next().unwrap_or("/") {
        "/" | "/index.html" => INDEX,
        "/dnd.html" => DND,
        "/file.html" => FILE,
        "/ime.html" => IME,
        "/yt.html" => YT,
        "/store.html" => STORE,
        "/ipc.html" => IPC_TOP,
        "/ipc-frame.html" => IPC_FRAME,
        _ => return None,
    };
    Some(format!("{HEAD}{}", body.replace("{BASE}", base)))
}

fn page_response(path: &str, base: &str) -> Response<Cow<'static, [u8]>> {
    match page(path, base) {
        Some(html) => Response::builder().header("Content-Type", "text/html; charset=utf-8").body(Cow::Owned(html.into_bytes())).unwrap(),
        None => Response::builder().status(404).body(Cow::Borrowed(&b"not found"[..])).unwrap(),
    }
}

fn serve_pages() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("127.0.0.1 に bind できない");
    let base = format!("http://127.0.0.1:{}", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    let served = base.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Err(e) = respond(stream, &served) { log(format!("http {e}")); }
        }
    });
    base
}

fn respond(mut stream: TcpStream, base: &str) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (status, body) = match page(&path, base) { Some(html) => ("200 OK", html), None => ("404 Not Found", "not found".to_string()) };
    log(format!("http {status} {path}"));
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

// ---- AppKit の状態（first responder・key window）を読む。egui と WebView が focus を取り合うかの証拠 ----

fn first_responder(view: *mut AnyObject) -> (String, bool) {
    if view.is_null() { return ("-".into(), false); }
    unsafe {
        let window: *mut AnyObject = msg_send![view, window];
        if window.is_null() { return ("(窓なし)".into(), false); }
        let key: bool = msg_send![window, isKeyWindow];
        let responder: *mut AnyObject = msg_send![window, firstResponder];
        let name = if responder.is_null() { "nil".into() } else { (*responder).class().name().to_string_lossy().into_owned() };
        (name, key)
    }
}

// 本体 src/main.rs の activate_accesskit と同じ理由: AccessKit は最初の AX 問い合わせに placeholder しか返さないので、
// 起動時に自分で 1 回尋ねて egui の木を作らせる（外から来る最初の get_window_state が button を見られるように）。
fn activate_accesskit(view: *mut AnyObject) {
    if view.is_null() { return; }
    unsafe { let _: *mut AnyObject = msg_send![view, accessibilityChildren]; }
}

fn subviews(view: *mut AnyObject) -> Vec<*mut AnyObject> {
    if view.is_null() { return Vec::new(); }
    unsafe {
        let list: *mut AnyObject = msg_send![view, subviews];
        let count: usize = msg_send![list, count];
        (0..count).map(|i| msg_send![list, objectAtIndex: i]).collect()
    }
}

fn class_name(object: *mut AnyObject) -> String {
    if object.is_null() { return "nil".into(); }
    unsafe { (*object).class().name().to_string_lossy().into_owned() }
}

// ---- AX の実験（button「AX: WebView を木に足す」）----
// AccessKit（accesskit_macos 0.18.1 subclass.rs）は winit の view を動的 subclass にして accessibilityChildren / HitTest /
// FocusedUIElement を egui の木だけで答える。wry の child は同じ view の subview なので、そのままだと WKWebView が AX の木から消える。
// AccessKit の subclass の accessibilityChildren にもう 1 段かぶせて、egui の子の後ろに WebView を足す（hit test と focus は未対応）。
static AX_WEBVIEW: AtomicPtr<AnyObject> = AtomicPtr::new(std::ptr::null_mut());
static AX_CHILDREN: OnceLock<Imp> = OnceLock::new();

unsafe extern "C-unwind" fn ax_children(this: *mut AnyObject, cmd: Sel) -> *mut AnyObject {
    let inner: unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> *mut AnyObject = std::mem::transmute(*AX_CHILDREN.get().unwrap());
    let children = inner(this, cmd);
    let webview = AX_WEBVIEW.load(Ordering::Relaxed);
    if webview.is_null() { return children; }
    if children.is_null() { return msg_send![class!(NSArray), arrayWithObject: webview]; }
    msg_send![children, arrayByAddingObject: webview]
}

fn patch_ax_children(view: *mut AnyObject, webview: *mut AnyObject) -> String {
    if view.is_null() || webview.is_null() { return "view か WebView が無い".into(); }
    unsafe {
        let class = (*view).class();
        let Some(method) = class.instance_method(sel!(accessibilityChildren)) else { return "accessibilityChildren が無い".into() };
        if AX_CHILDREN.set(method.implementation()).is_err() { return "既に足してある".into(); }
        AX_WEBVIEW.store(webview, Ordering::Relaxed);
        method.set_implementation(std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> *mut AnyObject, Imp>(ax_children));
        format!("{} の accessibilityChildren に {} を足した", class.name().to_string_lossy(), class_name(webview))
    }
}

// ---- Edit menu の実験（button「Edit menu を足す」）----
// wry の child は performKeyEquivalent: が常に NO（Cmd+キーを WebView が受けない）。winit の既定 menu には Edit が無い。
// 標準の Edit menu（copy: / paste: / selectAll: … を first responder へ送る）を足すと WebView に届くかを測る。
fn nsstring(text: &str) -> *mut AnyObject {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
}

fn install_edit_menu() -> String {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let main: *mut AnyObject = msg_send![app, mainMenu];
        if main.is_null() { return "mainMenu が無い".into(); }
        let menu: Retained<AnyObject> = msg_send![class!(NSMenu), new];
        let _: () = msg_send![&*menu, setTitle: nsstring("Edit")];
        let items = [("Undo", sel!(undo:), "z"), ("Redo", sel!(redo:), "Z"), ("Cut", sel!(cut:), "x"), ("Copy", sel!(copy:), "c"),
            ("Paste", sel!(paste:), "v"), ("Select All", sel!(selectAll:), "a")];
        for (title, action, key) in items {
            let _: *mut AnyObject = msg_send![&*menu, addItemWithTitle: nsstring(title), action: action, keyEquivalent: nsstring(key)];
        }
        let item: Retained<AnyObject> = msg_send![class!(NSMenuItem), new];
        let _: () = msg_send![&*item, setTitle: nsstring("Edit")];
        let _: () = msg_send![&*item, setSubmenu: &*menu];
        let _: () = msg_send![main, addItem: &*item];
        let count: isize = msg_send![main, numberOfItems];
        format!("Edit menu を足した（main menu の項目 {count}）")
    }
}

fn fonts(ctx: &egui::Context) {
    let Ok(data) = std::fs::read("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc") else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("jp".into(), egui::FontData::from_owned(data).into());
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "jp".into());
    fonts.families.entry(egui::FontFamily::Monospace).or_default().push("jp".into());
    ctx.set_fonts(fonts);
}

// ---- 項目 2〜5: eframe の窓 + child の WebView ----

enum Act { Load(String), LoadReferer(&'static str), Html(String), Js(&'static str), Probe(&'static str, &'static str), FocusWeb, FocusEgui, Toggle, Handoff,
    EditMenu, AxPatch, Stores }

fn buttons(base: &str) -> Vec<(&'static str, Act)> {
    vec![
        ("example.com", Act::Load("https://example.com/".into())),
        ("WebGL（get.webgl.org）", Act::Load("https://get.webgl.org/".into())),
        ("WebGL を JS で調べる", Act::Probe("webgl", WEBGL_PROBE)),
        ("drag & drop の頁", Act::Load(format!("{base}/dnd.html"))),
        ("file input の頁", Act::Load(format!("{base}/file.html"))),
        ("file input を JS で click", Act::Js("document.getElementById('f').click()")),
        ("IME の頁", Act::Load(format!("{base}/ime.html"))),
        ("WebView に focus（IME 欄）", Act::FocusWeb),
        ("IME 欄を JS で調べる", Act::Probe("field", FIELD_PROBE)),
        ("egui に focus", Act::FocusEgui),
        ("YouTube（http://127.0.0.1）", Act::Load(format!("{base}/yt.html"))),
        ("YouTube（kpoc://）", Act::Load("kpoc://localhost/yt.html".into())),
        ("YouTube embed（Referer: app.kangae.desktop）", Act::LoadReferer(YT_EMBED)),
        ("YouTube embed（Referer なし・top-level）", Act::Load(YT_EMBED.into())),
        ("YouTube embed（with_html・baseURL None）", Act::Html(yt_html())),
        ("YouTube 再生（JS）", Act::Js("player&&player.playVideo()")),
        ("YouTube 停止（JS）", Act::Js("player&&player.pauseVideo()")),
        ("Google sign-in", Act::Load("https://accounts.google.com/".into())),
        ("頁を JS で調べる（自前の頁だけ）", Act::Probe("page", PAGE_PROBE)),
        ("data store の頁（kpoc://）", Act::Load("kpoc://localhost/store.html".into())),
        ("data store の一覧", Act::Stores),
        ("IPC: 別 origin の iframe（kpoc://）", Act::Load("kpoc://localhost/ipc.html".into())),
        ("Edit menu を足す", Act::EditMenu),
        ("AX: WebView を木に足す", Act::AxPatch),
        ("WebView を隠す / 出す", Act::Toggle),
        ("外部 browser で開く（handoff）", Act::Handoff),
    ]
}

#[derive(Clone, Copy, Debug)]
struct Flags { no_webview: bool, autoplay: bool, store: Option<[u8; 16]> }

struct Poc {
    webview: Option<WebView>,
    ns_view: *mut AnyObject,
    base: String,
    applied: Option<[f32; 4]>,
    screen: Option<egui::Rect>,
    responder: (String, bool),
    frame_no: u64,
    note: String,
    focus_note: bool,
    stress: bool,
    overlay: bool,
    hidden: bool,
    beat: Instant,
}

impl Poc {
    fn new(cc: &eframe::CreationContext<'_>, base: String, flags: Flags) -> Self {
        let served = base.clone();
        fonts(&cc.egui_ctx);
        let ns_view = match cc.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::AppKit(h)) => h.ns_view.as_ptr().cast::<AnyObject>(),
            _ => std::ptr::null_mut(),
        };
        activate_accesskit(ns_view);
        let webview = if flags.no_webview {
            log("--no-webview: WebView を作らない（memory / CPU の基準）");
            None
        } else {
            let mut builder = WebViewBuilder::new()
                .with_url("https://example.com/")
                .with_bounds(Rect { position: LogicalPosition::new(300.0, 0.0).into(), size: LogicalSize::new(800.0, 600.0).into() })
                .with_autoplay(flags.autoplay)
                .with_devtools(true)
                .with_ipc_handler(|req: Request<String>| log(format!("ipc uri {} {}", req.uri(), req.body())))
                .with_custom_protocol("kpoc".into(), move |_id, req: Request<Vec<u8>>| {
                    log(format!("kpoc {}", req.uri()));
                    page_response(req.uri().path(), &served)
                })
                .with_on_page_load_handler(|event, url| {
                    #[allow(unreachable_patterns)]
                    let event = match event { PageLoadEvent::Started => "started", PageLoadEvent::Finished => "finished", _ => "other" };
                    log(format!("page_load {event} {url}"))
                })
                .with_navigation_handler(|url| { log(format!("navigate {url}")); true })
                .with_new_window_req_handler(|url, _features: NewWindowFeatures| {
                    log(format!("new_window_req {url} → Deny（別窓は開かない）"));
                    NewWindowResponse::Deny
                })
                .with_drag_drop_handler(|event: DragDropEvent| { log(format!("drag_drop {event:?}")); false })
                .with_document_title_changed_handler(|title| log(format!("title {title}")))
                .with_on_web_content_process_terminate_handler(|| log("WebContent process が終了した"));
            if let Some(id) = flags.store {
                log(format!("data store identifier {}", id.iter().map(|b| format!("{b:02x}")).collect::<String>()));
                builder = builder.with_data_store_identifier(id);
            }
            match builder.build_as_child(cc) {
                Ok(webview) => { log("WebView: build_as_child ok"); Some(webview) }
                Err(e) => { log(format!("WebView: build_as_child 失敗 {e}")); None }
            }
        };
        log(format!("winit の view {}・subview {:?}", class_name(ns_view), subviews(ns_view).iter().map(|v| class_name(*v)).collect::<Vec<_>>()));
        Self { webview, ns_view, base, applied: None, screen: None, responder: (String::new(), false), frame_no: 0,
            note: String::new(), focus_note: false, stress: false, overlay: false, hidden: false, beat: Instant::now() }
    }

    fn act(&mut self, label: &str, act: Act) {
        log(format!("button {label}"));
        if matches!(act, Act::FocusEgui) { self.focus_note = true; }
        if matches!(act, Act::Toggle) { self.hidden = !self.hidden; }
        let hidden = self.hidden;
        match act {
            Act::EditMenu => return log(format!("  {}", install_edit_menu())),
            Act::AxPatch => {
                let webview = subviews(self.ns_view).into_iter().find(|v| class_name(*v).contains("WebView")).unwrap_or(std::ptr::null_mut());
                return log(format!("  {}", patch_ax_children(self.ns_view, webview)));
            }
            Act::Stores => {
                let listed = WebView::fetch_data_store_identifiers(|ids| log(format!("  data store {} 個 {:?}", ids.len(),
                    ids.iter().map(|id| id.iter().map(|b| format!("{b:02x}")).collect::<String>()).collect::<Vec<_>>())));
                if let Err(e) = listed { log(format!("  error {e}")); }
                return;
            }
            _ => {}
        }
        let Some(webview) = self.webview.as_ref() else { log("  WebView なし（--no-webview）"); return };
        let result: Result<(), wry::Error> = match act {
            Act::Load(url) => webview.load_url(&url),
            Act::LoadReferer(url) => {
                let mut headers = HeaderMap::new();
                headers.insert(REFERER, HeaderValue::from_static(KANGAE_REFERER));
                webview.load_url_with_headers(url, headers)
            }
            Act::Html(html) => webview.load_html(&html),
            Act::Js(js) => webview.evaluate_script(js),
            Act::Probe(tag, js) => webview.evaluate_script_with_callback(js, move |out| log(format!("probe {tag} {out}"))),
            Act::FocusWeb => webview.focus().and_then(|_| webview.evaluate_script("var e=document.getElementById('ime');if(e)e.focus()")),
            Act::FocusEgui => webview.focus_parent(),
            Act::Toggle => webview.set_visible(!hidden),
            Act::Handoff => {
                // v0.13 の「WebView 不可なら外部 browser へ」の口。macOS の `open` = LaunchServices で既定の browser に渡す
                let url = webview.url().unwrap_or_default();
                match std::process::Command::new("open").arg(&url).status() {
                    Ok(status) => log(format!("handoff open {url} → {status}")),
                    Err(e) => log(format!("handoff open {url} 失敗 {e}")),
                }
                Ok(())
            }
            Act::EditMenu | Act::AxPatch | Act::Stores => Ok(()),
        };
        if let Err(e) = result { log(format!("  error {e}")); }
    }

    // egui の CentralPanel の rect（egui の point）を WebView の bounds（AppKit の logical point）へ。
    // macOS の child は autoresizing が上端固定だけ（wry: ViewMinYMargin）なので、窓の大きさが変わるたびに打ち直す。
    fn sync_bounds(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        let Some(webview) = self.webview.as_ref() else { return };
        let z = ctx.zoom_factor();
        let r = [rect.min.x * z, rect.min.y * z, rect.width() * z, rect.height() * z];
        if self.applied == Some(r) { return; }
        let set = webview.set_bounds(Rect {
            position: LogicalPosition::new(r[0] as f64, r[1] as f64).into(),
            size: LogicalSize::new(r[2] as f64, r[3] as f64).into(),
        });
        let back = webview.bounds().map(|b| format!("{:?} {:?}", b.position, b.size)).unwrap_or_else(|e| e.to_string());
        log(format!("set_bounds x{:.0} y{:.0} {:.0}×{:.0}（frame {}）→ {:?}・読み戻し {back}", r[0], r[1], r[2], r[3], self.frame_no,
            set.map_err(|e| e.to_string())));
        self.applied = Some(r);
    }
}

impl eframe::App for Poc {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame_no += 1;
        // NSOpenPanel の runModal 中に egui が描かれ続けるかを見る（毎 frame repaint の時だけ 2 秒ごと）
        if self.stress && self.beat.elapsed() >= Duration::from_secs(2) {
            log(format!("heartbeat frame {}", self.frame_no));
            self.beat = Instant::now();
        }
        let responder = first_responder(self.ns_view);
        if responder != self.responder {
            log(format!("first_responder {} → {}（key window {}・frame {}）", self.responder.0, responder.0, responder.1, self.frame_no));
            self.responder = responder;
        }
        let screen = ctx.screen_rect();
        if self.screen != Some(screen) {
            log(format!("window {:.0}×{:.0} pt（frame {}）", screen.width(), screen.height(), self.frame_no));
            self.screen = Some(screen);
        }
        let mut clicked: Option<(&'static str, Act)> = None;
        egui::SidePanel::left("ops").exact_width(300.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("WebView PoC");
                ui.label(format!("pid {}・試験頁 {}", std::process::id(), self.base));
                for (label, act) in buttons(&self.base) {
                    if ui.button(label).clicked() { clicked = Some((label, act)); }
                }
                ui.separator();
                let heading = ui.label("egui の入力欄");
                let field = ui.add(egui::TextEdit::singleline(&mut self.note).hint_text("egui 側の文字")).labelled_by(heading.id);
                if std::mem::take(&mut self.focus_note) { field.request_focus(); }
                if field.gained_focus() { log("egui TextEdit: focus を得た"); }
                if field.lost_focus() { log("egui TextEdit: focus を失った"); }
                if field.changed() { log(format!("egui TextEdit: {:?}", self.note)); }
                ui.checkbox(&mut self.stress, "毎 frame repaint（focus の取り合いを見る）");
                ui.checkbox(&mut self.overlay, "egui の浮き窓を WebView の上に出す");
                ui.separator();
                ui.monospace(format!("first responder: {}\nkey window: {}\nframe: {}", self.responder.0, self.responder.1, self.frame_no));
                if let Some(a) = self.applied { ui.monospace(format!("WebView: x{:.0} y{:.0} {:.0}×{:.0}", a[0], a[1], a[2], a[3])); }
                ui.separator();
                let recent: Vec<String> = RECENT.lock().unwrap().iter().rev().cloned().collect();
                for line in recent { ui.label(egui::RichText::new(line).small().monospace()); }
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::default()).show(ctx, |ui| {
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, egui::Color32::from_gray(228));
            let text = if self.webview.is_some() { "WebView の領域（これが見えたら WebView が重なっていない）" } else { "WebView なし（--no-webview）" };
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(15.0), egui::Color32::DARK_GRAY);
            self.sync_bounds(ctx, rect);
        });
        // WebView は GL の面より上の NSView。egui の popup / 浮き窓が WebView の範囲に重なった時にどちらが上に見えるかを撮る
        if self.overlay {
            egui::Window::new("egui の浮き窓").default_pos(egui::pos2(160.0, 300.0)).default_size([320.0, 120.0]).show(ctx, |ui| {
                ui.label("WebView の範囲に重ねた egui の窓。半分が見えなければ WebView の下に隠れている");
            });
        }
        if let Some((label, act)) = clicked { self.act(label, act); }
        if self.stress { ctx.request_repaint(); } else { ctx.request_repaint_after(Duration::from_millis(250)); }
    }
}

fn child(flags: Flags) -> eframe::Result<()> {
    let base = serve_pages();
    log(format!("試験頁 {base}（kpoc://localhost/ でも同じ頁）・{flags:?}"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kangae PoC — WebView child")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([800.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native("poc-webview", options, Box::new(move |cc| Ok(Box::new(Poc::new(cc, base, flags)))))
}

// ---- 項目 1: 単独の窓（winit 0.30）に WebView。egui を挟まない基準 ----

struct Standalone {
    url: String,
    webview: Option<WebView>, // window より先に drop する（field の宣言順）
    window: Option<winit::window::Window>,
}

impl winit::application::ApplicationHandler for Standalone {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_some() { return; }
        let attrs = winit::window::Window::default_attributes()
            .with_title("Kangae PoC — WebView standalone")
            .with_inner_size(winit::dpi::LogicalSize::new(1000.0, 700.0));
        let window = event_loop.create_window(attrs).expect("winit の窓を作れない");
        let built = WebViewBuilder::new()
            .with_url(self.url.as_str())
            .with_ipc_handler(|req: Request<String>| log(format!("ipc {}", req.body())))
            .with_on_page_load_handler(|event, url| {
                #[allow(unreachable_patterns)]
                let event = match event { PageLoadEvent::Started => "started", PageLoadEvent::Finished => "finished", _ => "other" };
                log(format!("page_load {event} {url}"))
            })
            .with_document_title_changed_handler(|title| log(format!("title {title}")))
            .build(&window);
        match built {
            Ok(webview) => { log("WebView: build ok"); self.webview = Some(webview); }
            Err(e) => log(format!("WebView: build 失敗 {e}")),
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, _id: winit::window::WindowId, event: winit::event::WindowEvent) {
        if let winit::event::WindowEvent::CloseRequested = event { event_loop.exit(); }
    }
}

fn standalone(url: String) {
    let event_loop = winit::event_loop::EventLoop::new().expect("event loop を作れない");
    let mut app = Standalone { url, webview: None, window: None };
    if let Err(e) = event_loop.run_app(&mut app) { log(format!("event loop {e}")); }
}

fn main() {
    START.get_or_init(Instant::now);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    if let Some(path) = value("--log") {
        *LOG_FILE.lock().unwrap() = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
    }
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    log(format!("start pid {} epoch {epoch} args {args:?}", std::process::id()));
    match args.first().map(String::as_str) {
        Some("standalone") => {
            let url = args.get(1).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "https://example.com/".into());
            standalone(url);
        }
        _ => {
            // --store NAME: NAME の先頭 16 byte を data store の identifier に（CD-25 (1)。space × software の代わり）
            let store = value("--store").map(|name| {
                let mut id = [0u8; 16];
                for (d, s) in id.iter_mut().zip(name.bytes()) { *d = s; }
                id
            });
            let flags = Flags { no_webview: has("--no-webview"), autoplay: has("--autoplay"), store };
            if let Err(e) = child(flags) { log(format!("eframe {e}")); }
        }
    }
}
