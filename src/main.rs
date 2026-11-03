mod adapters;
#[allow(dead_code, unused_imports)] mod agent; // AGT-001〜014（RQ5 で app.rs の Suggestions に配線・heuristics の一部は tests/agent.rs だけが呼ぶ）
mod app;
mod audio; // 鼻歌の録音・試聴・REAPER の insert / undo（G88b から Page の録音の帯が使う・kangae-rec / kangae-surface も）
mod capability_engine; // W19-2: Shibubu Engine の pipeline（発見・再利用・生成・検証・registry）。module 頭の allow(dead_code) あり
mod codegen; // Codegen Engine（今は kangae-codegen から使う。module 頭の allow(dead_code) あり）
mod crypto; // sha256 / base64 / 乱数（src/google と runtime が共有・CD-31）
mod composer; // unit C: Goal → App（generated.rs の Generate から呼ぶ）
mod domain;
mod engine;
use engine::{layout, ui_tree}; // ponytail: 互換（app.rs が crate::layout / crate::ui_tree を名指す）・R-ui で app.rs と一緒に消す
mod generated; // 一言目 → composer → runtime → renderer（統合 2 の通し）
#[allow(dead_code)] mod flags; // 層⑦ feature flag。統合役が app に配線するまで未使用
mod gui; // W2c: AI の GUI 操作の契約の型（口は adapters::mac_gui::MacGui の observe / act / verify）。W17-2 で bridge に配線するまで未使用（module 頭の allow(dead_code) あり）
mod github;
mod google; // Gmail / Calendar の OAuth と API（bin kangae-google と共用）。adapters::google が包む
#[allow(dead_code)] mod intel; // Software Intelligence（今は kangae-intel から使う）。統合役が app に配線するまで未使用
mod model;
mod terminal; // W4: Project が持つ本物の PTY（TerminalManager）。Generated への配線は W4 の実装
mod project; // W2: Project の Page の移り（router）と Page ごとの入力途中の状態。generated.rs が使う
mod mcp; // MCP 連携（~/.shibubu/mcp.json から読み込み・Claude MCP の引き継ぎ）
mod llm; // LLM provider（complete_or_local → complete_json → 検証。module 頭の allow(dead_code) あり）
mod llm_cli; // W13: 利用者が login 済みの LLM の CLI（claude -p）を PTY で起こす provider（製品は鍵を持たない。module 頭の allow(dead_code) あり）
mod own_usage; // USAGE-OWN-1101: Shibubu の中だけで使った AI の量（自分の呼び出しの記録＋Shibubu の Terminal の agent の token）
#[allow(dead_code)] mod media; // Music.app と YouTube（今は kangae-media から使う）。統合役が app に配線するまで未使用
mod repos;
#[allow(dead_code, unused_imports)] mod runtime; // RUN-001〜008・CORE-003 の呼び出し経路。generated.rs が local action を呼ぶ（agent と外部 adapter の配線は次の片で、それまで一部が未使用）
#[allow(dead_code, unused_imports)] mod storage; // CORE-009〜012。generated.rs が kangae.sqlite3 を開く（secret_store・log は次の片で、それまで一部が未使用）
#[cfg_attr(not(feature = "native_direct"), allow(dead_code, unused_imports))] mod surface; // v0.14 Native Direct Docking の型と bridge client（docs/diagrams.md §18）。統合役が app に配線するまで未使用
mod ui;
/// W19-2 の配線: Page の Engine の製品の実装（capability_engine）と拒否・実行履歴の文言。file は src/ui に置く（check_ui_copy の対象）が、ui/mod.rs には入れない
/// （tests/renderer.rs など ui だけを #[path] で引く試験の crate は capability_engine を持たず compile できない・W19a2 の全 suite で赤）
#[path = "ui/engine_page.rs"]
mod engine_page;
/// RQ1: Settings の画面（General・AI & Models・About）。engine_page と同じ理由で ui/mod.rs に入れない（model::UiSettings と llm::Budget を読む）
#[path = "ui/settings.rs"]
mod settings;

// AccessKit builds the AX tree lazily: the first `accessibilityChildren` query on the winit view only creates a
// placeholder (accesskit_macos adapter.rs `State::Placeholder`); egui's real tree arrives one frame later via
// `InitialTreeRequested` → `enable_accesskit` → repaint. A one-shot scanner (cua-driver `get_window_state`) therefore
// saw 76 chrome/menu elements and no widget. Ask for the children ourselves once at startup (eframe has installed the
// adapter before the app creator runs) so every external first query already sees the full tree. No crate: raw libobjc.
#[cfg(target_os="macos")]
fn activate_accesskit() {
    use std::ffi::{c_void,CStr};
    unsafe {
        // Pointer-returning messages only: on arm64/x86_64 this is the plain C calling convention objc2 itself relies on.
        let send:unsafe extern "C" fn(*mut c_void,*mut c_void)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let sel=|s:&CStr|sel_registerName(s.as_ptr());
        let app=send(objc_getClass(c"NSApplication".as_ptr()),sel(c"sharedApplication"));
        let window=send(send(app,sel(c"windows")),sel(c"firstObject"));
        if window.is_null(){return}
        let view=send(window,sel(c"contentView"));
        if !view.is_null(){send(view,sel(c"accessibilityChildren"));}
    }
}
#[cfg(not(target_os="macos"))]
fn activate_accesskit(){}

// libobjc の口（この file の objc の差し替えと呼び出しが全部ここを使う・新しい crate なし）。`objc_msgSend` は呼ぶ所ごとに型を付けて transmute する
#[cfg(target_os="macos")]
unsafe extern "C" {
    fn objc_getClass(name:*const std::ffi::c_char)->*mut std::ffi::c_void; fn sel_registerName(name:*const std::ffi::c_char)->*mut std::ffi::c_void; fn objc_msgSend();
    fn class_addMethod(cls:*mut std::ffi::c_void,sel:*mut std::ffi::c_void,imp:*const std::ffi::c_void,types:*const std::ffi::c_char)->u8;
    fn class_getInstanceMethod(cls:*mut std::ffi::c_void,sel:*mut std::ffi::c_void)->*mut std::ffi::c_void; fn method_getTypeEncoding(m:*mut std::ffi::c_void)->*const std::ffi::c_char;
    fn class_replaceMethod(cls:*mut std::ffi::c_void,sel:*mut std::ffi::c_void,imp:*const std::ffi::c_void,types:*const std::ffi::c_char)->*const std::ffi::c_void;
}

/// ADD-FILE（HOME-ATTACH-1101 追記 3・owner 198 通目の 7「addphotoorfile 押しても何も起きない」）: Shibubu の process の中で macOS の開く panel（`NSOpenPanel`）を
/// modal で開き、選んだ file・folder の path を返す（取り消し = 空の Ok）。前は `osascript` の別 process の JXA で、panel が Shibubu の窓の後ろに開き・失敗も黙っていた。
/// file・写真・zip・folder を 1 つの panel で・いくつでも。`message` は panel の上の 1 行・`prompt` は決める button の語。Err = 開けなかった理由（呼ぶ側が toast に 1 行）。
/// main thread（egui の update の中 = `Intent::Attach`）から呼ぶ: `runModal` が閉じるまで frame は止まる（modal の panel なので UI は待ってよい）。新しい crate なし（上の libobjc の口）。
/// 保証しない: Shibubu が前面でない時（AX で押した背景の試験）に panel を前へ出す事（`activateIgnoringOtherApps:` は W2b で何もしない実装に替えてある）。
#[cfg(target_os="macos")]
pub(crate) fn open_panel(message:&str,prompt:&str)->Result<Vec<std::path::PathBuf>,String>{
    use std::ffi::{c_char,c_void,CStr,CString};
    let (message,prompt)=(CString::new(message).map_err(|e|e.to_string())?,CString::new(prompt).map_err(|e|e.to_string())?);
    unsafe{
        // 呼ぶ所ごとに型を付けた objc_msgSend（引数と返り値は pointer・整数・BOOL〈x86_64 は signed char・arm64 は bool = どちらも 1 byte〉だけ）
        let send:unsafe extern "C" fn(*mut c_void,*mut c_void)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let send_str:unsafe extern "C" fn(*mut c_void,*mut c_void,*const c_char)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let set_obj:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void)=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let set_flag:unsafe extern "C" fn(*mut c_void,*mut c_void,i8)=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let run:unsafe extern "C" fn(*mut c_void,*mut c_void)->isize=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let count:unsafe extern "C" fn(*mut c_void,*mut c_void)->usize=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let at:unsafe extern "C" fn(*mut c_void,*mut c_void,usize)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let utf8:unsafe extern "C" fn(*mut c_void,*mut c_void)->*const c_char=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let sel=|s:&CStr|sel_registerName(s.as_ptr());
        let class=objc_getClass(c"NSOpenPanel".as_ptr());
        let panel=if class.is_null(){class}else{send(class,sel(c"openPanel"))};
        if panel.is_null(){return Err("macOS didn't give Shibubu a file picker".into())}
        let string=|s:&CStr|send_str(objc_getClass(c"NSString".as_ptr()),sel(c"stringWithUTF8String:"),s.as_ptr());
        for flag in [c"setCanChooseFiles:",c"setCanChooseDirectories:",c"setAllowsMultipleSelection:"]{set_flag(panel,sel(flag),1);}
        set_obj(panel,sel(c"setMessage:"),string(message.as_c_str()));
        set_obj(panel,sel(c"setPrompt:"),string(prompt.as_c_str()));
        // NSModalResponseOK = 1（Cancel = 0）
        if run(panel,sel(c"runModal"))!=1{return Ok(Vec::new())}
        let urls=send(panel,sel(c"URLs"));
        if urls.is_null(){return Ok(Vec::new())}
        let mut out=Vec::new();
        for i in 0..count(urls,sel(c"count")){
            let path=send(at(urls,sel(c"objectAtIndex:"),i),sel(c"path"));
            let p=if path.is_null(){std::ptr::null()}else{utf8(path,sel(c"UTF8String"))};
            if !p.is_null(){out.push(std::path::PathBuf::from(CStr::from_ptr(p).to_string_lossy().into_owned()));}
        }
        Ok(out)
    }
}
#[cfg(not(target_os="macos"))]
pub(crate) fn open_panel(_message:&str,_prompt:&str)->Result<Vec<std::path::PathBuf>,String>{Err("adding files needs macOS".into())}

/// MIRROR-FRAME-1101: 空の Mirror の枠の `Open an app…`（`ui::Startup::pick_app` に起動の時に入れる）。開く panel（`open_panel`）で選んだ最初の物の
/// `Contents/Info.plist` の `CFBundleIdentifier`（`plutil`）。取り消し = None・.app でない・読めない = Some(Err(1 行))。
#[cfg(feature="mirror")]
fn choose_app()->Option<Result<String,String>>{
    let picked=match open_panel("Choose an app to mirror","Mirror"){Ok(v)=>v.into_iter().next()?,Err(e)=>return Some(Err(e))};
    let out=std::process::Command::new("/usr/bin/plutil").args(["-extract","CFBundleIdentifier","raw","-o","-"]).arg(picked.join("Contents/Info.plist")).stdin(std::process::Stdio::null()).output().ok().filter(|o|o.status.success());
    let id=out.map(|o|String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|id|!id.is_empty());
    Some(id.ok_or_else(||"Shibubu can't tell which app this is · choose one from Applications".to_owned()))
}

/// W2b: 起動で前面を奪わない（W2 D-f 01-launch: `open -g` でも前面が Orca → shibubu に変わった）。
/// 原因: winit 0.30.13 は起動の時に `activateIgnoringOtherApps(true)` を呼ぶ（platform_impl/macos/app_state.rs:137・既定 true）。
/// 契約: `options.event_loop_builder` に `EventLoopBuilderExtMacOS::with_activate_ignoring_other_apps(false)` を入れる。
/// - Finder / Dock から開いた人: LaunchServices が前面にする（今と同じ）
/// - `open -g` / 背景で起こした試験: 前面の app は変わらない（YC_WAVES の W2b D-f で前後の前面 app を記録して確かめる）
/// - 実装で `winit = "0.30"`（Cargo.lock に在る 0.30.13・新しい crate ではない）を macOS の target 依存に足す
/// - **この 1 か所では足りなかった**（W2b の実機・pid 50227 の system log）: false でも `:137` の前面化の依頼は出る。窓 0 枚の間は
///   WindowServer が拒否して保留し、eframe が最初の描画で窓を出す時（epi_integration.rs:310 `set_visible(true)` →
///   window_delegate.rs:904 `makeKeyAndOrderFront`）に通って前面が替わった（起動から約 0.7 秒）。
///
/// 契約（W2b-fix・`suppress_activation`）:
/// - 人が普通に起動した時（Finder・Dock・`-g` 無しの `open`・`cargo run`）は前に出る。環境変数が無ければ何も替えない
/// - `open -g --env SHIBUBU_BACKGROUND=1` で起こした時（背景の実機試験）は、前面の app が変わらない。
///   `main()` が event loop を作る前に `NSApplication` の `activateIgnoringOtherApps:` を何もしない実装に替え、
///   `:137` と `focus_window`（window_delegate.rs:1580）の依頼そのものを消す。窓を出す動作は替えない
/// - `open -g` を app から知る公開 API は無いので、区別は試験側が渡す環境変数だけで行う
///
/// 契約（W2b-fix2・窓の重なり）: 前面を取らなくても窓は owner の窓の上に重なった（owner「5も出た」・system log は
/// dontMakeFrontmost=1 で前面化なし・00:40:33 と 00:41:07）。非 active の app でも `makeKeyAndOrderFront:` は窓を全 app の窓の上に並べる
/// （最初の描画 → `set_visible(true)` → window_delegate.rs:904-906）。
/// - `SHIBUBU_BACKGROUND=1` の時は、窓を owner の窓の下（全 app の一番後ろ）に置き、前面にも重ねない。AX の操作はできる。
///   app の中の key の窓にはする（W16M-2・pid 宛ての key を受ける。前面・他の app の窓の上下・owner の keyboard focus は変えない）
/// - 普通の起動は今まで通り（環境変数が無ければ何も替えない）
///
/// 契約（W2b-AX・AX の窓が常に 1 個在る）: `SHIBUBU_BACKGROUND=1` で起こした Shibubu は、CGWindowList で窓が onscreen の間、
/// 外からの `kAXWindows`（`AXUIElementCreateApplication(pid)`）が 10 秒以内に 1 個以上を返す（入力欄に文字を入れた後・保存の前後も）。
/// - 呼ぶ側の検査: `scripts/qa/bg/front-guard ax-sample` → `ax-judge`（detached = 答えたが 0 個 → 赤・unresponsive = -25204 だけ → 未測）
/// - 1 分の load が論理 CPU 数を超えても同じ（owner 2026-10-21「製品の不具合を前提に動いて」: 利用者の Mac も混む。混雑は再現の条件で、赤の言い訳にしない）
/// - 確かめた原因（axfix: runningboardd の log・`ps -M`・`sample`・probe で数えた `flushBuffer`）: **App Nap**。窓は起動から後ろ（`order_back_instead`）で
///   覆われ、最初の描画から 36〜60 秒で WindowServer が AppDrawing の assertion を落とすと、RunningBoard が `AppNap active:Y priority:Y timer:Tier5` にし、
///   main thread の優先度が 46 → 4 に落ちる。load 30〜175 では優先度 4 の main thread に CPU が回らず、1 frame の GL の flush
///   （`flushBuffer` → IOAccel / SkyLight の `mach_msg`・普段 1ms）が 1〜38 秒かかった。app は隠れて idle でも 230ms ごとに frame を頼んでいた
///   （app.rs の常時の pump・4〜5 frame/秒）ので main thread はその flush の中に居続け、AX の問い（kAXWindows・cua-driver の木）が後ろに並んだ
///   （-25204・要素 0・type_text の拒否）。integ の 4 run で App Nap の始まりと AX が答えなくなった時刻は 1 秒以内で一致。
///   同じ binary に App Nap を外す activity だけを差し込んだ run（probe・2 本・load 63〜110）は main thread 46 → 28 で、AX 92/92・打った文字を読み戻せた
/// - 欄の focus（AXPress）は引き金ではない（どの run でも App Nap の始まりと同じ頃に当たっただけ）: cursor の点滅は窓に focus が無いので回らない
///   （egui text_edit/builder.rs:764-776）・IME の `invalidateCharacterCoordinates` と accesskit の通知は focus と打鍵ごとに 1〜3 回・frame は focus の前後とも 4.6/秒
/// - 直し（W2b-AX 直し）: `keep_out_of_app_nap`（下の契約）と、idle の frame を 0 にする（app.rs `next_repaint`・背景の thread の結果は `Tx::send` が起こす・
///   動いている timer と「N 分前」は描く側が替わる時の frame を頼む・Page の Terminal の出力と shell の終わりは PTY の側が起こす = `Generated::wake_on_terminal`）。背景の起動だけに在った `vsync=false` は外した
///   （試験の道だけが人の起動と違う GL で測っていた・反証の指摘 2。外した build の BG の測りは docs/evidence/W2b-AX/log.md の「axfix-term」）
#[cfg(target_os="macos")]
fn launch_without_stealing_front(options:&mut eframe::NativeOptions) {
    use winit::platform::macos::EventLoopBuilderExtMacOS;
    options.event_loop_builder=Some(Box::new(|b|{b.with_activate_ignoring_other_apps(false);}));
}
#[cfg(not(target_os="macos"))]
fn launch_without_stealing_front(_options:&mut eframe::NativeOptions){}

/// FX1（FX-001）: ⌘Q・Dock の Quit・logout（NSApp の `terminate:`）が来た印。`ask_before_quit` が立て、app.rs の `update` が `swap(false)` で読んで
/// `Intent::Quit { anyway: false }` に通す（窓の閉じる button と同じ道）。
/// Wave: FX1
static QUIT_ASKED:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);

/// FX1（FX-001）: 終わる前に app へ聞く口。winit 0.30.13 は既定の menu の Quit を `terminate:` に結び（platform_impl/macos/menu.rs:71）、
/// `applicationShouldTerminate:` を持たず `applicationWillTerminate:` → そのまま終わる（app_state.rs:166）ので、egui の `close_requested` /
/// `ViewportCommand::CancelClose` では ⌘Q を止められない（止まるのは窓の閉じる button だけ）。
/// 契約: winit の app delegate の class に `applicationShouldTerminate:` を足し（`ignore_activation` と同じ objc の差し替え・新しい crate なし）、
/// `QUIT_ASKED` を立てて `ctx.request_repaint()`・止める答えを返す → app が保存して `ViewportCommand::Close` で自分で終わる。
/// 止める答えは `NSTerminateCancel`。`NSTerminateLater` は返事までの間 AppKit が modal の run loop で回り、egui の frame（保存と 1 行）が
/// 回る事を確かめていないので使わない。logout・再起動・電源を切る（quit の Apple Event の理由・`terminate_reply`）は止めない（`NSTerminateNow`）。
/// 例: `eframe::run_native("Shibubu", options, Box::new(|cc| { ask_before_quit(&cc.egui_ctx); Ok(Box::new(app::ShibubuApp::new(cc))) }))`
/// 保証しない: SIGKILL・crash・電源断。macOS 以外では何もしない。winit の delegate の class が無い（winit の版が替わった）時は stderr に 1 行で、⌘Q は今まで通り黙って終わる。
/// Wave: FX1
#[cfg(target_os="macos")]
fn ask_before_quit(ctx:&eframe::egui::Context){
    use std::ffi::c_void;
    let _=QUIT_CTX.set(ctx.clone());
    unsafe {
        // winit 0.30.13 platform_impl/macos/app_state.rs の `declare_class!` の NAME（EventLoop を作った後 = run_native の app creator の中なら在る）
        let cls=objc_getClass(c"WinitApplicationDelegate".as_ptr());
        if cls.is_null(){eprintln!("shibubu: couldn't hook Quit (no WinitApplicationDelegate) · ⌘Q quits without asking");return}
        // NSApplicationTerminateReply (NSUInteger) applicationShouldTerminate:(NSApplication *)sender = "Q@:@"
        class_addMethod(cls,sel_registerName(c"applicationShouldTerminate:".as_ptr()),should_terminate as *const c_void,c"Q@:@".as_ptr());
        // delegate は EventLoop が既に NSApp に渡した後: 渡し直して、AppKit が delegate の持つ method を数え直すようにする（pointer の引数と返り値だけ・activate_accesskit と同じ呼び方）
        let send:unsafe extern "C" fn(*mut c_void,*mut c_void)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let set:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void)=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let app=send(objc_getClass(c"NSApplication".as_ptr()),sel_registerName(c"sharedApplication".as_ptr()));
        let delegate=send(app,sel_registerName(c"delegate".as_ptr()));
        if !delegate.is_null(){
            set(app,sel_registerName(c"setDelegate:".as_ptr()),std::ptr::null_mut());
            set(app,sel_registerName(c"setDelegate:".as_ptr()),delegate);
        }
    }
}
/// `ask_before_quit` が起こす frame の宛先（`applicationShouldTerminate:` は egui の外から呼ばれる）
#[cfg(target_os="macos")]
static QUIT_CTX:std::sync::OnceLock<eframe::egui::Context>=std::sync::OnceLock::new();
/// `applicationShouldTerminate:` の実装。logout・再起動・電源を切る（quit の Apple Event の理由 `kAEQuitReason`・`terminate_reply`）は
/// `NSTerminateNow`（= 1）でそのまま終わる（winit の `applicationWillTerminate:` → eframe の `exiting` → `on_exit` の `save_now`・失敗は stderr = FX1 の前と同じ）。
/// それ以外（⌘Q の menu・Dock の Quit・`NSRunningApplication.terminate`）は印を立てて frame を起こし `NSTerminateCancel`（= 0）。
/// 終わるのは app が保存して `ViewportCommand::Close` を送った時
#[cfg(target_os="macos")]
extern "C" fn should_terminate(_delegate:*mut std::ffi::c_void,_cmd:*mut std::ffi::c_void,_sender:*mut std::ffi::c_void)->usize{
    let reply=terminate_reply(quit_reason());
    if reply==0 {
        QUIT_ASKED.store(true,std::sync::atomic::Ordering::SeqCst);
        if let Some(ctx)=QUIT_CTX.get(){ctx.request_repaint();}
    }
    reply
}

/// FX1 反証の直し: 今 AppKit が処理している quit の Apple Event の理由（`kAEQuitReason` = 'why?'・AERegistry.h）。Apple Event で来ていない（⌘Q の menu）・
/// 理由の無い quit（Dock・`NSRunningApplication.terminate`）は None。attribute を先に見て、無ければ parameter（header は parameter と書き、attribute で読む例もあるので両方）。
/// 保証しない: loginwindow が実際にどちらで送るか（logout の Apple Event はこの Mac で未測）。
/// Wave: FX1
#[cfg(target_os="macos")]
fn quit_reason()->Option<u32>{
    use std::ffi::c_void;
    const WHY:u32=u32::from_be_bytes(*b"why?");
    unsafe {
        let send:unsafe extern "C" fn(*mut c_void,*mut c_void)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let by_key:unsafe extern "C" fn(*mut c_void,*mut c_void,u32)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let code:unsafe extern "C" fn(*mut c_void,*mut c_void)->u32=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let manager=send(objc_getClass(c"NSAppleEventManager".as_ptr()),sel_registerName(c"sharedAppleEventManager".as_ptr()));
        if manager.is_null(){return None}
        let event=send(manager,sel_registerName(c"currentAppleEvent".as_ptr()));
        if event.is_null(){return None}
        let mut why=by_key(event,sel_registerName(c"attributeDescriptorForKeyword:".as_ptr()),WHY);
        if why.is_null(){why=by_key(event,sel_registerName(c"paramDescriptorForKeyword:".as_ptr()),WHY);}
        (!why.is_null()).then(||code(why,sel_registerName(c"typeCodeValue".as_ptr())))
    }
}

/// FX1 反証の直し: quit の理由 → `applicationShouldTerminate:` の答え。logout・再起動・電源を切る（AERegistry.h の kAEReallyLogOut 'rlgo'・
/// kAELogOut 'logo'・kAERestart 'rest'・kAEShutDown 'shut'・kAEQuitAll 'quia'）は `NSTerminateNow`（1）= 止めない。理由が無い・知らない理由は
/// `NSTerminateCancel`（0）= 止めて app が聞く。
/// ponytail: logout の間に保存できなければ stderr の 1 行だけ（FX1 の前と同じ）。止めて聞くなら NSTerminateLater と replyToApplicationShouldTerminate: を
/// winit の handler の外（CFRunLoopTimer）から返す形（modal の run loop で egui の frame が回る事を BG で確かめてから）。
/// Wave: FX1
fn terminate_reply(reason:Option<u32>)->usize{
    const SESSION_END:[&[u8;4];5]=[b"rlgo",b"logo",b"rest",b"shut",b"quia"];
    match reason{Some(r) if SESSION_END.iter().any(|c|u32::from_be_bytes(**c)==r)=>1,_=>0}
}
#[cfg(not(target_os="macos"))]
fn ask_before_quit(_ctx:&eframe::egui::Context){}

/// 背景の起動の印（上の契約）。試験側が `open -g --env SHIBUBU_BACKGROUND=1` で渡す。
const BACKGROUND_ENV:&str="SHIBUBU_BACKGROUND";

/// `activateIgnoringOtherApps:` の差し替え先。BOOL は arm64 で bool・x86_64 で signed char、どちらも 1 byte の整数 register なので u8 で受ける。
#[cfg(target_os="macos")]
extern "C" fn ignore_activation(_app:*mut std::ffi::c_void,_cmd:*mut std::ffi::c_void,_flag:u8){}

/// `makeKeyAndOrderFront:` / `orderFront:`（どちらも `(id sender)->void`）の差し替え先: 前に並べず、key にもせず `orderBack:` で一番後ろに出す。
/// 既に出ている窓（`isVisible`）は並べ替えない（owner 10-30「orcaを押すとmirror先のtexteditだけが浮き出て」: 後から来た前に出す要求〈Mirror の Focus〉まで
/// 一番後ろへ送り、Shibubu の窓の下にしまった本物だけが見えた。契約は「前面と他の app の窓の上下は変えない」）。
#[cfg(target_os="macos")]
extern "C" fn order_back_instead(window:*mut std::ffi::c_void,_cmd:*mut std::ffi::c_void,sender:*mut std::ffi::c_void) {
    use std::ffi::c_void;
    unsafe {
        let visible:unsafe extern "C" fn(*mut c_void,*mut c_void)->i8=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        if visible(window,sel_registerName(c"isVisible".as_ptr()))!=0 {
            return;
        }
        let send:unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void)=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(window,sel_registerName(c"orderBack:".as_ptr()),sender);
    }
}

/// `makeKeyAndOrderFront:` の差し替え先（W16M-2）: `orderBack:` で一番後ろに出し、app の中で key の窓にはする（`makeKeyWindow`・前に並べず
/// activate もしない）。key の窓が無いと pid 宛ての key（人の手の代わりの postToPid）を AppKit が捨て、背景の Shibubu に文字が届かない（W16M-2 で実測:
/// 欄にも Mirror の矩形にも 0 文字・AX の main / focused の窓が nil）。AX の `AXMain = true` もこの method を通る。
#[cfg(target_os="macos")]
extern "C" fn order_back_and_make_key(window:*mut std::ffi::c_void,cmd:*mut std::ffi::c_void,sender:*mut std::ffi::c_void) {
    use std::ffi::c_void;
    order_back_instead(window,cmd,sender);
    unsafe {
        let send:unsafe extern "C" fn(*mut c_void,*mut c_void)=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(window,sel_registerName(c"makeKeyWindow".as_ptr()));
    }
}

/// 上の 2 つの契約の実装: `NSApplication` の `activateIgnoringOtherApps:` を何もしない実装に、
/// `NSWindow`（winit の WinitWindow は継ぐだけで上書きしない）の `orderFront:` を `orderBack:` に、`makeKeyAndOrderFront:` を `orderBack:` と
/// `makeKeyWindow` に替える（前面と他の app の窓の上下は変えず、pid 宛ての key は受ける）。型の符号は元の method から写す。
#[cfg(target_os="macos")]
fn suppress_activation() {
    use std::ffi::{c_void,CStr};
    let replace=|class:&CStr,name:&CStr,imp:*const c_void| unsafe {
        let cls=objc_getClass(class.as_ptr());
        if cls.is_null(){return}
        let sel=sel_registerName(name.as_ptr());
        let m=class_getInstanceMethod(cls,sel);
        if m.is_null(){return}
        class_replaceMethod(cls,sel,imp,method_getTypeEncoding(m));
    };
    replace(c"NSApplication",c"activateIgnoringOtherApps:",ignore_activation as *const c_void);
    replace(c"NSWindow",c"makeKeyAndOrderFront:",order_back_and_make_key as *const c_void);
    replace(c"NSWindow",c"orderFront:",order_back_instead as *const c_void);
}
#[cfg(not(target_os="macos"))]
fn suppress_activation(){}

/// 契約（axfix・App Nap）: どの起動の道でも（人の起動も `SHIBUBU_BACKGROUND=1` も）process の一生の間 App Nap に入らない =
/// 窓が後ろ・別の Space・隠れていても main thread の優先度は落ちず、AX と支援技術（cua-driver・VoiceOver）の問いに答え続ける（上の W2b-AX の原因）。
/// - 実装: `run_native` の前に `[[NSProcessInfo processInfo] beginActivityWithOptions:NSActivityUserInitiatedAllowingIdleSystemSleep reason:]` を 1 回。
///   返った activity は retain して手放さない（`endActivity:` を呼ばない）。system の idle sleep は止めない（AllowingIdleSystemSleep）
/// - 払う物: App Nap が省いていた電力。idle の frame を 0 にして返す（app.rs `next_repaint`・どの View でも: Project の Page の動いている Terminal も
///   出力の無い間は頼まない = `Generated::wake_on_terminal`）。例外は Page に WebView / Native Direct の面が在る間の 230ms（app.rs `SURFACE_POLL`）。cursor の点滅・IME・描画は替えない
/// - 確かめ方（試験の側）: `run.sh start` から 120 秒（AppDrawing が落ちるのは 36〜60 秒）`ps -M -p <pid>` の 1 行目（main thread）の PRI が 20 以上・
///   runningboardd の log が `Set AppNap state … priority:N`。直す前の build（axfix-pre）は 46 → 4・`priority:Y`
/// 返り値: activity を受け取れたか（false = 受け取れず App Nap のまま）。
#[cfg(target_os="macos")]
fn keep_out_of_app_nap()->bool {
    use std::ffi::{c_char,c_void,CStr};
    // NSProcessInfo.h: NSActivityUserInitiated（0x00FFFFFF | NSActivityIdleSystemSleepDisabled）から IdleSystemSleepDisabled（1 << 20）を外した物
    const USER_INITIATED_ALLOWING_IDLE_SYSTEM_SLEEP:u64=0x00FF_FFFF&!(1<<20);
    unsafe {
        // 引数は pointer と整数だけ（arm64 / x86_64 とも C の呼び方のまま・activate_accesskit と同じ）
        let send:unsafe extern "C" fn(*mut c_void,*mut c_void)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let send_cstr:unsafe extern "C" fn(*mut c_void,*mut c_void,*const c_char)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let begin:unsafe extern "C" fn(*mut c_void,*mut c_void,u64,*mut c_void)->*mut c_void=std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let sel=|s:&CStr|sel_registerName(s.as_ptr());
        // autorelease pool はまだ無いので alloc + init（stringWithUTF8String: は pool 無しで autorelease する）
        let reason=send_cstr(send(objc_getClass(c"NSString".as_ptr()),sel(c"alloc")),sel(c"initWithUTF8String:"),c"Shibubu answers accessibility (AX, VoiceOver) while in the background".as_ptr());
        let info=send(objc_getClass(c"NSProcessInfo".as_ptr()),sel(c"processInfo"));
        let activity=begin(info,sel(c"beginActivityWithOptions:reason:"),USER_INITIATED_ALLOWING_IDLE_SYSTEM_SLEEP,reason);
        if activity.is_null(){return false}
        send(activity,sel(c"retain"));
        true
    }
}
#[cfg(not(target_os="macos"))]
fn keep_out_of_app_nap()->bool{true}

/// 窓と描画の選択（G89）。DESIGN.md §1.1: 1440×900 が基準。§8 の最小段は 960。
/// 契約（G89）: 描画は wgpu（Metal）= `eframe::Renderer::Wgpu`。glow（OpenGL）は Cargo.toml の eframe の feature から外した
/// （10-27 の GPU 再起動 2 回はどちらも main thread が `-[NSOpenGLContext flushBuffer]` で待っていた）。
/// 例: `let mut options=native_options(); launch_without_stealing_front(&mut options);`。保証しない: Metal の無い Mac（macOS 10.13 以前）で起動する事。
/// 契約（TITLEBAR-ICONS-1101）: 窓の中身は題の帯の下まで（`with_fullsize_content_view`・帯は透明・題の字は隠す）。header（`theme::HEADER_H`）が題の帯の 1 段。
/// Wave: M2, TITLEBAR-ICONS-1101
fn native_options()->eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Shibubu")
            .with_inner_size([1440.0,900.0])
            .with_min_inner_size([960.0,640.0])
            // TITLEBAR-ICONS-1101（owner 216「赤黄緑の browser のところにおいて」）: 中身を題の帯の下まで伸ばし・題の帯を透明に・題の字を隠す = header（36）が題の帯そのもの。
            // 赤黄緑は残す（AppKitOp::TrafficLights が帯の縦の中央へ）・窓の題（Window menu・Mission Control の名前）は with_title のまま
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            // ICON-RUN-1101（owner 197）: 無いと eframe が起動の時に既定の「e」を Dock に置き .app の icns を上書きする。黒の背景のレモン = icns と同じ
            .with_icon(std::sync::Arc::new(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/shibubu-icon-black-1024.png")).expect("icon"))),
        renderer:eframe::Renderer::Wgpu,
        ..Default::default()
    }
}

fn main()->eframe::Result<()> {
    // W9: Project の Terminal の中の `shibubu project pages|context|surfaces [--json]` は窓を出さずに答えて終わる（docs/diagrams.md §31）
    let args:Vec<String>=std::env::args_os().map(|a|a.to_string_lossy().into_owned()).collect();
    if let Some(code)=project::context::cli_main(&args){std::process::exit(code)}
    if !keep_out_of_app_nap(){eprintln!("shibubu: App Nap を外せませんでした（背景・隠れた窓では AX の応答が遅れます）");}
    // 起動の時に渡す物（generated と ui は試験の crate にも入り、audio・llm_cli・開く panel を引けないので、ここで 1 回だけ渡す）:
    // app と web の icon（TITLEBAR-ICONS / PROVIDER-ICONS）・空の枠の `Open an app…`（MIRROR-FRAME-1101）・sign in の tab の command（SIGNIN-1101）・
    // Grok と Kimi Code の使用量（STATUSBAR-1101 追記 2）
    generated::set_startup(generated::Startup{
        ui:ui::Startup{app_icon:audio::surface::app_icon_png,site_icon:audio::surface::site_icon_png,#[cfg(feature="mirror")]pick_app:choose_app},
        sign_in_command:llm_cli::sign_in_command,
        cli_usage:generated::CliUsageReader{clis:&llm_cli::USAGE_CLIS,dir:llm_cli::cli_account_dir,read:llm_cli::cli_usage},
    });
    let mut options=native_options();
    launch_without_stealing_front(&mut options);
    // 背景の起動の印が替えるのは前面を取らない事だけ。描画（wgpu・vsync）は人の起動と同じ（試験の道と製品の道を揃える・W2b-AX の反証の指摘 2）
    if std::env::var_os(BACKGROUND_ENV).is_some(){suppress_activation();}
    eframe::run_native("Shibubu",options,Box::new(|cc|{activate_accesskit();ask_before_quit(&cc.egui_ctx);ui::skeleton::install_dev_overlay(&cc.egui_ctx);Ok(Box::new(app::ShibubuApp::new(cc)))}))
}

#[cfg(all(test,target_os="macos"))]
mod tests {
    use super::{objc_getClass,sel_registerName};
    use std::ffi::c_void;
    unsafe extern "C" { fn class_getMethodImplementation(cls:*mut c_void,sel:*mut c_void)->*const c_void; }

    // 契約: 背景の起動では activateIgnoringOtherApps: が差し替わっている（winit の :137 と focus_window がここを通る）
    #[test]
    fn background_launch_replaces_activation() {
        unsafe {
            let cls=objc_getClass(c"NSApplication".as_ptr());
            assert!(!cls.is_null(),"NSApplication が見つからない");
            let sel=sel_registerName(c"activateIgnoringOtherApps:".as_ptr());
            super::suppress_activation();
            assert_eq!(class_getMethodImplementation(cls,sel),super::ignore_activation as *const c_void);
        }
    }

    // FX1 反証の直し: logout・再起動・電源を切る（loginwindow の quit の理由 = AERegistry.h の kAEReallyLogOut 'rlgo'・kAELogOut 'logo'・
    // kAERestart 'rest'・kAEShutDown 'shut'・kAEQuitAll 'quia'）は止めない（NSTerminateNow = 1）。理由の無い quit（⌘Q の menu・Dock・
    // NSRunningApplication.terminate）と知らない理由は今まで通り止めて聞く（NSTerminateCancel = 0）。
    // 負の対照: 対応表を常に 0 にする（2df6b10 の形 = 毎回 Cancel）→ 赤。
    #[test]
    fn fx1_session_end_is_not_cancelled_but_plain_quit_asks() {
        let code=|s:&[u8;4]|u32::from_be_bytes(*s);
        for why in [b"rlgo",b"logo",b"rest",b"shut",b"quia"] {
            assert_eq!(super::terminate_reply(Some(code(why))),1,"{} で logout / 再起動を止めた",String::from_utf8_lossy(why));
        }
        assert_eq!(super::terminate_reply(None),0,"理由の無い quit（⌘Q・Dock）を聞かずに終わらせた");
        assert_eq!(super::terminate_reply(Some(code(b"zzzz"))),0,"知らない理由を聞かずに終わらせた");
    }

    // 契約（axfix）: App Nap を外す activity を受け取れる（selector と引数の写し違いは例外で止まるか null）。効き目（main thread の PRI）は試験の側の ps -M
    #[test]
    fn w2b_ax_app_nap_activity_is_taken() {
        assert!(super::keep_out_of_app_nap(),"beginActivityWithOptions:reason: が activity を返さない");
    }

    // 契約: 背景の起動では窓を前に並べる 2 つ（window_delegate.rs:790-792・:906・:1581 と app_state.rs:441 が通る）が orderBack: に替わっている
    #[test]
    fn background_launch_orders_window_back() {
        unsafe {
            let cls=objc_getClass(c"NSWindow".as_ptr());
            assert!(!cls.is_null(),"NSWindow が見つからない");
            super::suppress_activation();
            // W16M-2: makeKeyAndOrderFront: は orderBack: に加えて key の窓にする（背景の Shibubu が pid 宛ての key を受ける）
            for (name,imp) in [(c"makeKeyAndOrderFront:",super::order_back_and_make_key as *const c_void),(c"orderFront:",super::order_back_instead as *const c_void)] {
                assert_eq!(class_getMethodImplementation(cls,sel_registerName(name.as_ptr())),imp,"{name:?} が差し替わっていない");
            }
        }
    }
}

// 試験の module はここから後ろ: scripts/interfaces.py と check_app_agnostic.py は最初の `#[cfg(test)]` の行から後ろを読まない（上に置くと main.rs の残りが索引と境界の検査から抜ける）
#[cfg(test)] mod app_wfe_attack; // WF-E 反証 9。src/*.rs は門の lint が main.rs の mod を要求する（include だけだと crate の外）

/// G89: 描画は wgpu（Metal）・glow（OpenGL）は依存ごと無い。
/// 負の対照（commit message）: Cargo.toml の eframe を default-features = true に戻す → 行の検査で赤（renderer の行を消せば既定の Glow で赤）。
#[cfg(test)]
mod g89_tests {
    #[test]
    fn g89_the_app_draws_with_wgpu_not_opengl() {
        assert_eq!(super::native_options().renderer,eframe::Renderer::Wgpu,"描画が wgpu（Metal）でない");
        let line=include_str!("../Cargo.toml").lines().find(|l|l.starts_with("eframe")).expect("Cargo.toml に eframe の行が無い");
        assert!(line.contains("default-features = false")&&line.contains("\"wgpu\"")&&!line.contains("glow"),"eframe の feature に glow が入りうる: {line}");
    }
    /// TITLEBAR-ICONS-1101（owner 216）: 窓の中身は題の帯の下まで・帯は透明・題の字は出さない（header が題の帯の 1 段）。赤黄緑は残す。
    /// 負の対照: 3 つの with_* のどれかを外す → 赤。
    #[test]
    fn titlebar1101_the_content_runs_under_a_transparent_title_bar() {
        let v=super::native_options().viewport;
        assert_eq!((v.fullsize_content_view,v.titlebar_shown,v.title_shown,v.titlebar_buttons_shown),(Some(true),Some(false),Some(false),None),"題の帯の設定: {v:?}");
    }
}

/// W12a の通し（画面なし・本物の git・Core は `ResourceReader` だけを見る）: 一言 → Project → 開く → Adapter の registry が git で読み、
/// Dev の Page の Git の欄が引く `ui_state(main.{app}, "resource.repo")` が試験の側の git と一致 → README を変えて「↻ 読み直す」→ 変更 file に出る。
/// 試験であって実機確認ではない（AX の木の照合は scripts/qa/bg の git_seen・実機の段）。
#[cfg(test)]
mod w12a_tests {
    use crate::generated::Generated;
    use crate::ui::shell::{PageIntent,View};
    use std::path::Path;
    use std::process::Command;

    fn sh_git(dir:&Path,args:&[&str])->String {
        let o=Command::new("git").arg("-C").arg(dir).args(args)
            .env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1")
            .env("GIT_AUTHOR_NAME","qa").env("GIT_AUTHOR_EMAIL","qa@localhost").env("GIT_COMMITTER_NAME","qa").env("GIT_COMMITTER_EMAIL","qa@localhost")
            .output().unwrap();
        assert!(o.status.success(),"git {args:?}: {}",String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_owned()
    }

    #[test]
    fn real_repo_reaches_the_dev_page_state_through_the_adapter_registry() {
        let root=std::env::temp_dir().join(format!("shibubu-w12a-main-{}",crate::storage::new_id()));
        let repo=root.join("shibubu-yc-demo");
        std::fs::create_dir_all(&repo).unwrap();
        sh_git(&repo,&["init","-q","-b","main"]);
        std::fs::write(repo.join("README.md"),"yc\n").unwrap();
        sh_git(&repo,&["add","README.md"]);
        sh_git(&repo,&["commit","-q","-m","yc-initial"]);
        let mut g=Generated::open(&root.join("data"));
        g.set_resource_reader(Box::new(crate::adapters::AdapterRegistry::with_local_adapters()));
        let said="この repo の開発環境を作って。README は TextEdit で直して、構成図は Excalidraw で描いて repo に保存したい";
        let View::Project{project,page}=g.create_project(said,Some(&repo),None).unwrap().unwrap() else {panic!("Project の View ではない")};
        g.open_project(&project,Some(&page)).unwrap();
        let app=g.app_of_project(&project).unwrap();
        let state=|g:&mut Generated|g.open_app(&app).unwrap().2.rt.ui_state("resource.repo").unwrap();
        let s=state(&mut g);
        assert_eq!(s["head"]["sha"],serde_json::json!(sh_git(&repo,&["rev-parse","HEAD"])),"試験の側の rev-parse HEAD と同じ 40 桁");
        assert_eq!(s["head"]["branch"],serde_json::json!(sh_git(&repo,&["branch","--show-current"])));
        assert_eq!(s["changes"],serde_json::json!([]),"clean（試験の側の status --porcelain も空）: {s}");
        assert!(sh_git(&repo,&["status","--porcelain"]).is_empty());
        // 正の対照: 試験の側が README に 1 行足す → ↻ 読み直す → 変更 file に README.md（試験の側の status と同じ）
        std::fs::write(repo.join("README.md"),"yc\nnonce-w12a\n").unwrap();
        assert_eq!(sh_git(&repo,&["status","--porcelain"])," M README.md".trim());
        g.page_intent(&project,PageIntent::Reread).unwrap();
        assert_eq!(state(&mut g)["changes"],serde_json::json!([{"status":" M","path":"README.md"}]));
        drop(g);
        let _=std::fs::remove_dir_all(&root);
    }
}
