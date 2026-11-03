// Kangae Live Mirror の helper（v0.14 §3.3・§11 P2・docs/diagrams.md §19）。1 file・package 依存なし:
//   swiftc -O -parse-as-library bridge/macos-mirror/KangaeMirror.swift -o <出力先>/kangae-mirror-helper
//
//   status                                   画面収録・Accessibility・event 送信の許可（preflight だけ。dialog を出さない）
//   request                                  画面収録の許可を求める（CGRequestScreenCaptureAccess。system の dialog が出る。人の button からだけ呼ぶ）
//   list [--all]                             capture できる窓（windowID・pid・bundle id・app 名・題名・frame・on_screen）。--all は別の Space の窓も
//   stream --window ID [--max-width N] [--fps F] [--discard] [--surfaces NAME] [--smooth auto|smooth|saver]
//                                            その窓だけを ScreenCaptureKit で取り、stdout に [tag u8][len u32 LE][payload] で流す。
//                                            tag 1 = frame（[header 長 u32 LE][header JSON][RGBA]）・tag 2 = 出来事の JSON（sent / error / stats / stopped）。
//                                            stdin の 1 行 1 JSON（入力の命令）を元の窓へ送る。stdin が閉じたら全部止めて終わる
//                                            G32: 1 本の process が N 本の stream を持つ（--window の窓 = stream 1）。stdin の "cmd" の行で足し・外し・窓を探す:
//                                              {"cmd":"open","stream":N,"window":ID}   同じ process の中に stream N を起こす（--max-width・--fps・--discard は起こした時の値）
//                                              {"cmd":"close","stream":N}              stream N を止める（{"kind":"stopped","stream":N,"reason":"closed"}）
//                                              {"cmd":"list","id":R,"all":true}        窓の一覧 → {"kind":"windows","id":R,"windows":[list と同じ形]}・
//                                                                                       取れない時は "windows" の代わりに "error"（no_screen_capture_permission・capture_timeout・shareable_failed）
//                                              {"cmd":"status","id":R}                 MIRROR-QUIET-1101: status と同じ許可 → {"kind":"answer","id":R,"screen_capture":…,"accessibility":…,"post_event":…}
//                                              {"cmd":"doc-path","id":R,"pid":P,"window":ID,"ask":false}
//                                                                                       MIRROR-QUIET-1101: doc-path と同じ答え → {"kind":"answer","id":R,"path":…,"reason":…}
//                                              {"cmd":"view","stream":N,"w":PX,"h":PX}  MIRROR-SPEED-1101: Shibubu がその面を描く大きさ（pixel）。撮る大きさ = 窓の pixel とこれの小さい方
//                                                                                       （縦横の比は窓のまま・窓より細かくは撮らない）。答えは無い（次の frame の w / h）・無い stream は bad_view
//                                              {"cmd":"shot","id":R,"windows":[ID…],"width":PX,"dir":D}
//                                                                                       MIRROR-FRAME-1101: 窓ごとの小さな絵（Choose a window の一覧の行）。SCShareableContent は 1 回・
//                                                                                       1 枚ずつ SCScreenshotManager（macOS 14.0+）で幅 PX に撮り D/<ID>.png に書く →
//                                                                                       {"kind":"answer","id":R,"shots":[撮れた ID…]}（撮れない窓は飛ばす）・許可が無い・取れない = "error"
//                                              {"cmd":"smoothness","mode":"auto"|"smooth"|"saver"}
//                                                                                       MIRROR-SPEED-1101: 撮る枚数（smooth = 60 / 秒・saver = 15・auto = 60 で始め、機械が混むと 30・15）。
//                                                                                       --fps を渡して起こした stream は替えない・知らない語は bad_smoothness
//                                            "cmd" の無い行は入力で、"stream":N の窓へ送る（無ければ 1）。frame の header と出来事には "stream":N が付く。
//                                            stream が 1 本止まっても（窓が消えた・-3805）process は終わらない（stopped は stream ごとに 1 回）。--window の窓が始まらない時だけ exit 3。
//                                            理由（G32 の probe）: 別の process の同じ実行 file が SCShareableContent を呼ぶと、生きている stream が -3805 で落ちる
//                                            （同じ process の中の 3 回と 2 本目の stream では落ちない 2/2・別の実行 file の list では落ちない）。
//                                            なので 2 面目の窓探しと stream は、1 面目の process の stdin に頼む（src/surface/mirror.rs の `HelperProc`）
//   serve [--max-width N] [--fps F]          MIRROR-QUIET-1101（owner 208）: stream と同じ stdin の口を窓 0 本で始める。Shibubu 1 回の起動で 1 本だけ常駐し、
//                                            窓探し（list）・status・doc-path・映す（open）を全部この process の stdin で受ける（探すたびに process を起こさない）。
//                                            stdin が閉じたら終わる。stream も serve も最初に {"kind":"ready","caps":["list","status","doc-path","open","shot"]} を 1 回出す
//                                            （"shot" は MIRROR-FRAME-1101 から。言わない版には Rust は小さな絵を頼まない = 行は灰の地）
//                                            （Rust の側はこれを見て stdin の status / doc-path を使う。言わない前の版・試験の偽物には今まで通り 1 回きりの process）。
//                                            AppKit を読み込まない（NSApplication を作ると launchservicesd に app として check in し、Contents/MacOS に居ると
//                                            Shibubu の bundle id で「もう 1 つの Shibubu」に見えた）。.app では Contents/Helpers に置き、埋め込みの Info.plist
//                                            （ai.shibubu.desktop.mirror・LSUIElement）を持つ（scripts/release/build-mac.sh）
//   --surfaces NAME（stream / serve）      MIRROR-SPEED-1101（owner 202・205）: frame の画素を pipe で送らず、SCK の IOSurface を mach port で Shibubu へ渡す（写し 0 回）。
//                                            NAME = Shibubu が bootstrap_check_in した名前（bootstrap_look_up で引く）。1 frame = 1 通の mach message:
//                                              header 24 byte: bits = COMPLEX | COPY_SEND（0x8000_0013）・size・remote = NAME の port・local 0・voucher 0・id = 0x5348
//                                              body: 記述子の数 1・port 記述子（IOSurfaceCreateMachPort の port・MOVE_SEND）・[len u32][frame の header JSON]（4 byte に詰める）
//                                              header JSON は tag 1 の header と同じ形（seq・w・h・captured_ms・sent_ms・stream・content・scale・factor）
//                                            送りは待たない（MACH_SEND_TIMEOUT 0）: 受け口が満ちている = その 1 枚を捨てる・受け口が無い（Shibubu が終わった）= 以後 pipe の tag 1。
//                                            NAME を引けない時も pipe のまま（前の版の Shibubu・試験の偽物）。Shibubu は送り手の pid を audit trailer で確かめる
//   既定（MIRROR-SPEED-1101）              --max-width は上限無し（撮る大きさは view と窓の pixel が決める）・--fps は無し = --smooth の枚数（既定 auto）・
//                                            SCK の queueDepth は --surfaces の時 5（Shibubu が描いている今と 1 つ前の 2 枚を持つ）・他は 3
//   input --window ID --pid PID '<JSON>'     入力を 1 つだけ送って結果の JSON を 1 行出す（stream 無しの試験用）
//   ax-scroll --pid PID [--title T]          その app の題名 T の窓（無ければ前の窓）の縦の scroll bar の値（0〜1・Accessibility）
//   ax-text --pid PID [--title 題名]         その窓の文書の今の中身（保存していない編集も含む・Accessibility）
//   ax-bounds --pid PID --window ID --loc L --len N [--title 題名]
//                                            その app の前の窓の文書の L 文字目から N 文字の矩形を、窓の中の割合（0〜1）で（Accessibility）
//
// 入力の JSON（x / y は元の窓の中の割合 0〜1。窓の位置は送るたびに CGWindowListCopyWindowInfo で引き直す）:
//   {"id":1,"op":"click","x":0.5,"y":0.5,"count":1,"via":"pid"}
//   {"id":2,"op":"scroll","x":0.5,"y":0.5,"dy":-120,"dx":0}                pixel（dy 負 = 下へ・dx は横）
//   {"id":3,"op":"drag","x":0.2,"y":0.3,"x2":0.6,"y2":0.3,"steps":8}
//   {"id":4,"op":"text","text":"こんにちは"}                                確定した文字をそのまま入れる（IME の変換中の文字は作れない）
//   {"id":5,"op":"key","key":"s","mods":["cmd"]}                           shortcut・矢印など
//   {"id":6,"op":"focus","title":"題名"}                                   Accessibility で窓を main・文書を focused に（activate はしない）
// 口の分離（YC W16M-0・Rust 側は src/surface/mirror.rs。検査は portReject・窓や許可を見る前に拒む）:
//   stream の stdin = 人の口: "origin":"human" と "relay_seq":N の無い行は送らず {"kind":"error","reason":"not_human_relay"}。
//                    "focus"（AX）は人の入力ではないので、この口では受けない（同じ reason）
//   input subcommand = 自動の口（measure と試験）: "origin":"human" の行は送らず {"kind":"error","reason":"human_origin_on_automated_port"}
// 人の口の道（W16M-2・Rust の `route_for` が決めて "route" に書く。helper はその道だけで送る・via と source は見ない）:
//   "post_to_pid"        text と key だけ。CGEvent.postToPid（背景の app に文字・矢印・⌃ / ⌘ の移動の key は届く・mouse は 0/4 なので mouse にこの道は無い）。
//                        key は app の key の窓に届くので、送る前に映している窓を AX で main にする（`focusMirroredWindow`・Accessibility の許可が要る）。
//                        AX は CGWindowID を公開 API で出さないので、映している窓は位置と大きさ＋題名（CG の kCGWindowName）で突き合わせ、CG の側と AX の
//                        側のどちらでも 1 つに決まる時だけ送る（`mirroredWindow`・同じ位置と大きさと題名の窓が 2 つ以上・AX に無い窓は needs_front）
//   "ax_at_point"        1 回の click だけ。AXUIElementCopyElementAtPosition（app の要素で引く＝他の app の窓に隠れていても映す窓の要素）→
//                        その要素の窓が `mirroredWindow` と同じ（CFEqual）時だけ、AXPress を持てば押す・文字の欄なら AXRangeForPosition の位置に
//                        AXSelectedTextRange を置く・どちらでもなければ needs_front
//   "save_document"      ⌘S だけ。映している窓の書類（document of window id <窓 id>）を、その pid の app に Apple Event の save で保存させる
//                        （app を前に出さない・宛先は bundle id でなく pid・書類は題名でなく窓で決める）
//   "front"              MIRROR-HYBRID-1101: click・drag・scroll だけ。Shibubu が本物の窓を枠の位置のまま前に出した後の、触った最初の 1 押し（Rust の `relay_front`）。
//                        点の一番上の窓（layer 0・見える物）が映している窓になるのを 20ms ごとに最長 500ms 待ち、なったら HID（via hid）で送る。
//                        ならなければ送らず not_front（別の窓の上に押さない）。Accessibility の許可が要る（無ければ not_trusted）
//   背景の app では menu の key equivalent（⌘S・⌘Z・⌘A…）を postToPid で送っても処理されず、menu の項目は AXEnabled が古いまま false で
//   AXPress しても何も起きない（2026-10-21 TextEdit で実測）。なので ⌘Z などは Rust の側で needs_front にして、ここへは来ない。
//   拒否の reason: bad_route（道が無い・op と合わない）・needs_front（その点に押せる物も文字の欄も無い・別の窓が前・映している窓が 1 つに決まらない・2 回以上の click・
//                  窓が書類を持たない・app が save を持たない）・not_trusted（Accessibility の許可が無い・post_to_pid と ax_at_point と front）・
//                  not_front（front の 500ms で本物が点の一番上に来なかった＝Rust の `NeedsFront`）・
//                  automation_denied（Apple Event の許可が無い・save_document）・ax_press_failed_<code> / ax_select_failed_<code>（app が AX の押す・
//                  選択に答えなかった＝Rust の `NoAnswer`）。答えの行には "route" を写す（Rust の `refusal`）
// source（任意）: "private"（既定）/ "hid" / "combined" = CGEventSource の state（背後の app に key が届くかを比べる）
// via: "pid"（既定）= CGEvent.postToPid。cursor を動かさず、その process にだけ届く。mouse は windowNumber を event に入れる。
//      "hid" = CGEvent.post(.cghidEventTap)。本物の cursor が動き、その点で一番上の窓が受ける（focus を奪う）。
//
// API の出典（Apple Developer Documentation の platforms。2026-10-18 取得）:
//   SCShareableContent・SCStream・SCContentFilter.init(desktopIndependentWindow:)・SCStreamConfiguration.showsCursor = macOS 12.3+
//   SCStreamConfiguration.ignoreShadowsSingleWindow = macOS 14.0+
//   CGPreflightScreenCaptureAccess・CGRequestScreenCaptureAccess = macOS 10.15+・CGEvent.postToPid = 10.11+・
//   CGEvent.keyboardSetUnicodeString = 10.4+（https://developer.apple.com/documentation/screencapturekit 等）
import Accelerate
import ApplicationServices
import Bootstrap
import CoreGraphics
import CoreMedia
import CoreVideo
import Foundation
import ImageIO
import IOSurface
import ScreenCaptureKit

// ============================== 出力 ==============================

let out = FileHandle.standardOutput
let outLock = NSLock()

func json(_ v: Any) -> Data { (try? JSONSerialization.data(withJSONObject: v, options: [.sortedKeys])) ?? Data("{}".utf8) }

func le32(_ n: Int) -> Data { var v = UInt32(n).littleEndian; return Data(bytes: &v, count: 4) }

/// tag つきの 1 通（frame と出来事が同じ stdout に混ざっても切れ目が分かる）
func message(_ tag: UInt8, _ payload: Data) {
    var d = Data([tag]); d.append(le32(payload.count)); d.append(payload)
    outLock.lock(); defer { outLock.unlock() }
    out.write(d)
}

func event(_ v: [String: Any]) { message(2, json(v)) }

func log(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }

func fail(_ s: String) -> Never { log(s); exit(2) }

func nowMs() -> Double { Date().timeIntervalSince1970 * 1000 }

/// mach absolute time（SCStreamFrameInfo.displayTime）→ unix ms
let machToMs: (UInt64) -> Double = {
    var tb = mach_timebase_info_data_t(); mach_timebase_info(&tb)
    let nowMach = mach_absolute_time(), nowUnix = nowMs()
    return { t in nowUnix - Double(Int64(nowMach) - Int64(t)) * Double(tb.numer) / Double(tb.denom) / 1e6 }
}()

func cpuMs() -> Double {
    var r = rusage(); getrusage(RUSAGE_SELF, &r)
    return Double(r.ru_utime.tv_sec + r.ru_stime.tv_sec) * 1000 + Double(r.ru_utime.tv_usec + r.ru_stime.tv_usec) / 1000
}

// ============================== 窓 ==============================

struct Bounds { let rect: CGRect; let pid: pid_t; let title: String? }

/// 今の窓の bounds（point・top-left 原点 = CGEvent と同じ座標）と持ち主の pid と題名（kCGWindowName・画面収録の許可が無ければ nil）。窓が無ければ nil
func bounds(_ id: CGWindowID) -> Bounds? {
    guard let list = CGWindowListCopyWindowInfo([.optionIncludingWindow], id) as? [[String: Any]],
          let w = list.first(where: { ($0[kCGWindowNumber as String] as? Int).map(CGWindowID.init) == id }),
          let b = w[kCGWindowBounds as String] as? NSDictionary,
          let rect = CGRect(dictionaryRepresentation: b as CFDictionary),
          let pid = w[kCGWindowOwnerPID as String] as? Int else { return nil }
    return Bounds(rect: rect, pid: pid_t(pid), title: w[kCGWindowName as String] as? String)
}

/// G90 (3): `bounds` が引けない窓の止まった理由。窓がまだ在れば（app が隠れた ⌘H・最小化 = optionIncludingWindow では 0 件・optionAll には在る）
/// window_hidden、無ければ window_gone。SCK は隠れた窓の frame を出さない（10-28 実測: suspended だけ・complete 0）ので映さず、Shibubu が `Show {App}` を出す
func goneReason(_ id: CGWindowID) -> String {
    let all = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
    return all.contains { ($0[kCGWindowNumber as String] as? Int).map(CGWindowID.init) == id } ? "window_hidden" : "window_gone"
}

struct Timeout: Error {}

/// 先に着いた 1 つだけを通す印（withTimeout の答えと期限のどちらか）
final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func claim() -> Bool { lock.lock(); defer { lock.unlock() }; if done { return false }; done = true; return true }
}

/// ScreenCaptureKit の daemon が混んでいると答えが返らない事がある（load 900 で実測: SCShareableContent が 6 分以上返らなかった）。期限で諦める。
/// W16M-3: task group は抜ける時に全部の子の終わりを待つので、取り消しに答えない SCShareableContent では期限が効かなかった（2026-10-23 実測: `list --all` が
/// 70 分返らず、それを待つ Shibubu の UI の thread が止まった）。先に着いた方で 1 回だけ返し、答えない呼び出しは置いていく（list と stream はその後 exit する）
func withTimeout<T: Sendable>(_ seconds: Double, _ op: @escaping @Sendable () async throws -> T) async throws -> T {
    let once = Once()
    return try await withCheckedThrowingContinuation { (c: CheckedContinuation<T, Error>) in
        Task {
            do { let v = try await op(); if once.claim() { c.resume(returning: v) } } catch { if once.claim() { c.resume(throwing: error) } }
        }
        Task {
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1e9))
            if once.claim() { c.resume(throwing: Timeout()) }
        }
    }
}

/// all = 別の Space の窓・隠れた窓も含める（窓を選んだ後の stream は、元の窓が画面に無くても引ける方がよい）
func shareable(all: Bool) async throws -> SCShareableContent {
    try await withTimeout(30) { try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: !all) }
}

func describe(_ w: SCWindow) -> [String: Any] {
    [
        "window_id": Int(w.windowID),
        "pid": Int(w.owningApplication?.processID ?? 0),
        "bundle_id": w.owningApplication?.bundleIdentifier ?? "",
        "app": w.owningApplication?.applicationName ?? "",
        "title": w.title ?? "",
        "frame": ["x": w.frame.origin.x, "y": w.frame.origin.y, "w": w.frame.width, "h": w.frame.height],
        "on_screen": w.isOnScreen,
        "layer": w.windowLayer,
    ]
}

// ============================== 入力 ==============================

let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15,
    "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31, "u": 32,
    "i": 34, "p": 35, "return": 36, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46, "tab": 48, "space": 49, "delete": 51, "escape": 53,
    "left": 123, "right": 124, "down": 125, "up": 126, "home": 115, "end": 119, "pageup": 116, "pagedown": 121, "forwarddelete": 117,
]

struct InputError: Error { let reason: String }

/// 入力を 1 つ送る。返すのは送った時刻と画面の点（届いたかは、この後の frame か読み戻しで別に確かめる）
func send(_ cmd: [String: Any], window: CGWindowID, pid expected: pid_t) throws -> [String: Any] {
    guard let b = bounds(window) else { throw InputError(reason: "window_gone") }
    guard b.pid == expected else { throw InputError(reason: "pid_changed") }
    let via = (cmd["via"] as? String) ?? "pid"
    guard via == "pid" || via == "hid" else { throw InputError(reason: "bad_via") }
    // source: private（既定。本物の keyboard の修飾 key と混ざらない）/ hid / combined（試験で比べる）
    let state: CGEventSourceStateID = ["hid": .hidSystemState, "combined": .combinedSessionState][(cmd["source"] as? String) ?? ""] ?? .privateState
    let src = CGEventSource(stateID: state)
    func point(_ kx: String, _ ky: String) throws -> CGPoint {
        guard let x = (cmd[kx] as? NSNumber)?.doubleValue, let y = (cmd[ky] as? NSNumber)?.doubleValue, (0...1).contains(x), (0...1).contains(y)
        else { throw InputError(reason: "bad_point") }
        return CGPoint(x: b.rect.minX + x * b.rect.width, y: b.rect.minY + y * b.rect.height)
    }
    func post(_ e: CGEvent?) throws {
        guard let e else { throw InputError(reason: "event_create_failed") }
        if e.type != .keyDown && e.type != .keyUp {
            e.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(window))
            e.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(window))
        }
        if via == "hid" { e.post(tap: .cghidEventTap) } else { e.postToPid(b.pid) }
    }
    func mouse(_ type: CGEventType, _ p: CGPoint, clicks: Int64 = 1) throws {
        let e = CGEvent(mouseEventSource: src, mouseType: type, mouseCursorPosition: p, mouseButton: .left)
        e?.setIntegerValueField(.mouseEventClickState, value: clicks)
        try post(e)
    }
    let op = (cmd["op"] as? String) ?? ""
    var at: CGPoint? = nil
    let sent = nowMs()
    switch op {
    case "click":
        let p = try point("x", "y"); at = p
        let count = max(1, (cmd["count"] as? Int) ?? 1)
        for n in 1...count {
            try mouse(.leftMouseDown, p, clicks: Int64(n)); try mouse(.leftMouseUp, p, clicks: Int64(n))
        }
    case "scroll":
        let p = try point("x", "y"); at = p
        let dy = Int32((cmd["dy"] as? NSNumber)?.intValue ?? 0), dx = Int32((cmd["dx"] as? NSNumber)?.intValue ?? 0)
        guard dy != 0 || dx != 0 else { throw InputError(reason: "bad_scroll") }
        let e = CGEvent(scrollWheelEvent2Source: src, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0)
        e?.location = p
        try post(e)
    case "drag":
        let a = try point("x", "y"), z = try point("x2", "y2"); at = a
        let steps = min(60, max(1, (cmd["steps"] as? Int) ?? 8))
        try mouse(.leftMouseDown, a)
        for i in 1...steps {
            let t = CGFloat(i) / CGFloat(steps)
            usleep(16_000)
            try mouse(.leftMouseDragged, CGPoint(x: a.x + (z.x - a.x) * t, y: a.y + (z.y - a.y) * t))
        }
        usleep(16_000)
        try mouse(.leftMouseUp, z)
    case "focus":
        // Accessibility で窓を main にし、その中の文書を focused にする（app を前に出す activate は呼ばない）
        guard let r = axFocus(pid: b.pid, title: cmd["title"] as? String) else { throw InputError(reason: "ax_focus_failed") }
        if r != "ok" { throw InputError(reason: r) }
    case "text":
        guard let text = cmd["text"] as? String, !text.isEmpty else { throw InputError(reason: "bad_text") }
        let units = Array(text.utf16)
        // 1 event に載る UTF-16 は 20 前後まで。surrogate pair を割らないように切る
        var i = 0
        while i < units.count {
            var j = min(i + 16, units.count)
            if j < units.count, UTF16.isLeadSurrogate(units[j - 1]) { j -= 1 }
            var chunk = Array(units[i..<j])
            for down in [true, false] {
                let e = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: down)
                e?.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: &chunk)
                try post(e)
            }
            i = j
        }
    case "key":
        guard let name = (cmd["key"] as? String)?.lowercased(), let code = keyCodes[name] else { throw InputError(reason: "bad_key") }
        var flags: CGEventFlags = []
        for m in (cmd["mods"] as? [String]) ?? [] {
            switch m {
            case "cmd": flags.insert(.maskCommand)
            case "shift": flags.insert(.maskShift)
            case "option": flags.insert(.maskAlternate)
            case "control": flags.insert(.maskControl)
            default: throw InputError(reason: "bad_mod")
            }
        }
        for down in [true, false] {
            let e = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down)
            e?.flags = flags
            try post(e)
        }
    default:
        throw InputError(reason: "bad_op")
    }
    var r: [String: Any] = ["kind": "sent", "op": op, "via": via, "sent_ms": sent, "window": ["x": b.rect.minX, "y": b.rect.minY, "w": b.rect.width, "h": b.rect.height]]
    if let id = cmd["id"] { r["id"] = id }
    if let at { r["point"] = ["x": at.x, "y": at.y] }
    return r
}

/// 口の検査（W16M-0）。拒む理由か nil。stream の stdin（human: true）は origin: human と relay_seq の在る行だけ・focus は拒む。
/// input subcommand（human: false）は origin: human を拒む。窓・pid・許可を見る前に呼ぶ
func portReject(_ cmd: [String: Any], human: Bool) -> [String: Any]? {
    let origin = cmd["origin"] as? String
    let reason: String?
    if human {
        reason = origin == "human" && cmd["relay_seq"] is NSNumber && (cmd["op"] as? String) != "focus" ? nil : "not_human_relay"
    } else {
        reason = origin == "human" ? "human_origin_on_automated_port" : nil
    }
    guard let reason else { return nil }
    var r: [String: Any] = ["kind": "error", "op": cmd["op"] ?? "", "reason": reason]
    if let id = cmd["id"] { r["id"] = id }
    return r
}

/// human = stream の stdin（道で送る `sendHuman`）・false = input subcommand（`send`）。答えには id と（在れば）route を写す
func sendReport(_ cmd: [String: Any], window: CGWindowID, pid: pid_t, human: Bool = false) -> [String: Any] {
    var r: [String: Any]
    do { r = human ? try sendHuman(cmd, window: window, pid: pid) : try send(cmd, window: window, pid: pid) } catch let e as InputError {
        r = ["kind": "error", "op": cmd["op"] ?? "", "reason": e.reason]
        if let id = cmd["id"] { r["id"] = id }
    } catch { r = ["kind": "error", "reason": "\(error)"] }
    if human, let route = cmd["route"] { r["route"] = route }
    return r
}

// ============================== 人の口の道（W16M-2） ==============================

enum Route: String { case postToPid = "post_to_pid", axAtPoint = "ax_at_point", saveDocument = "save_document", front = "front" }

/// MIRROR-HYBRID-1101: 点 `p`（画面の point）の一番上の窓（layer 0・見える物・Desktop の要素は除く）の番号。CGWindowList は手前から順
func topWindow(at p: CGPoint) -> CGWindowID? {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { return nil }
    for w in list {
        guard (w[kCGWindowLayer as String] as? Int) == 0, ((w[kCGWindowAlpha as String] as? Double) ?? 1) > 0.01,
              let d = w[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: d as CFDictionary), r.contains(p)
        else { continue }
        return (w[kCGWindowNumber as String] as? Int).map(CGWindowID.init)
    }
    return nil
}

/// MIRROR-HYBRID-1101: Shibubu が本物を前に出すのを待つ（点の一番上が `window` になるまで 20ms ごとに最長 500ms）。来なければ false（別の窓の上に押さない）
func waitFront(_ window: CGWindowID, at p: CGPoint) -> Bool {
    let end = Date().addingTimeInterval(0.5)
    repeat {
        if topWindow(at: p) == window { return true }
        usleep(20_000)
    } while Date() < end
    return false
}

func route(of cmd: [String: Any]) -> Route? { (cmd["route"] as? String).flatMap(Route.init(rawValue:)) }

/// 人の口の 1 行を、行の "route" の道だけで送る（上の頭の契約）。窓と pid は道より先に見る（消えた窓に AX で触らない）
func sendHuman(_ cmd: [String: Any], window: CGWindowID, pid expected: pid_t) throws -> [String: Any] {
    guard let r = route(of: cmd) else { throw InputError(reason: "bad_route") }
    let op = (cmd["op"] as? String) ?? ""
    guard let b = bounds(window) else { throw InputError(reason: "window_gone") }
    guard b.pid == expected else { throw InputError(reason: "pid_changed") }
    if case .postToPid = r {
        guard op == "text" || op == "key" else { throw InputError(reason: "bad_route") }
        try focusMirroredWindow(b)
        var c = cmd; c["via"] = "pid"; c.removeValue(forKey: "source")
        return try send(c, window: window, pid: expected)
    }
    if case .front = r {
        // MIRROR-HYBRID-1101: 触った最初の 1 押しを、前に出た本物へ HID で（本物の cursor は人が押した所に在る）。本物が点の一番上に来るまで待つ
        guard op == "click" || op == "drag" || op == "scroll" else { throw InputError(reason: "bad_route") }
        guard AXIsProcessTrusted() else { throw InputError(reason: "not_trusted") }
        guard let x = (cmd["x"] as? NSNumber)?.doubleValue, let y = (cmd["y"] as? NSNumber)?.doubleValue, (0...1).contains(x), (0...1).contains(y)
        else { throw InputError(reason: "bad_point") }
        guard waitFront(window, at: CGPoint(x: b.rect.minX + x * b.rect.width, y: b.rect.minY + y * b.rect.height)) else { throw InputError(reason: "not_front") }
        var c = cmd; c["via"] = "hid"; c.removeValue(forKey: "source")
        return try send(c, window: window, pid: expected)
    }
    let sent = nowMs()
    var out: [String: Any]
    switch r {
    case .axAtPoint:
        guard op == "click" else { throw InputError(reason: "bad_route") }
        guard AXIsProcessTrusted() else { throw InputError(reason: "not_trusted") }
        out = try axAtPoint(cmd, b)
    case .saveDocument:
        guard op == "key", (cmd["key"] as? String)?.lowercased() == "s", (cmd["mods"] as? [String]) == ["cmd"] else { throw InputError(reason: "bad_route") }
        out = try saveDocument(window: window, pid: b.pid)
    case .postToPid, .front:
        throw InputError(reason: "bad_route")
    }
    out["kind"] = "sent"; out["op"] = op; out["sent_ms"] = sent
    out["window"] = ["x": b.rect.minX, "y": b.rect.minY, "w": b.rect.width, "h": b.rect.height]
    if let id = cmd["id"] { out["id"] = id }
    return out
}

/// 要素の窓（要素が窓ならそれ自身）
func windowOf(_ e: AXUIElement) -> AXUIElement? {
    let w: AnyObject? = (axAttr(e, kAXRoleAttribute) as? String) == kAXWindowRole ? e : axAttr(e, kAXWindowAttribute)
    guard let w, CFGetTypeID(w) == AXUIElementGetTypeID() else { return nil }
    return (w as! AXUIElement)
}

/// AX の窓の位置と大きさが CG の bounds と同じか（2pt まで）
func sameFrame(_ w: AXUIElement, _ rect: CGRect) -> Bool {
    guard let pv = axAttr(w, kAXPositionAttribute), let sv = axAttr(w, kAXSizeAttribute) else { return false }
    var p = CGPoint.zero, s = CGSize.zero
    guard AXValueGetValue(pv as! AXValue, .cgPoint, &p), AXValueGetValue(sv as! AXValue, .cgSize, &s) else { return false }
    return abs(p.x - rect.minX) <= 2 && abs(p.y - rect.minY) <= 2 && abs(s.width - rect.width) <= 2 && abs(s.height - rect.height) <= 2
}

/// 映している窓（`b` = CGWindowID の今の bounds と題名）の AX の窓。AX は CGWindowID を公開 API で出さないので、位置と大きさ＋題名で突き合わせ、
/// CG の側でこの pid にその位置と大きさと題名の窓が 1 枚だけ（＝映している窓そのもの）、かつ AX の側でも 1 つだけの時に返す。
/// 同じ app の zoom した 2 枚・全画面・tab のような同じ位置と大きさの窓は題名で分け、題名まで同じ窓が在る時と AX に出ていない時（別の Space・
/// 裏の tab）は needs_front（別の書類に打たない・押さない）。題名が読めない（画面収録の許可が無い）時は位置と大きさだけで 1 つに決まる事を要る
func mirroredWindow(_ app: AXUIElement, _ b: Bounds) throws -> AXUIElement {
    let cg = ((CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]]) ?? []).filter { w in
        (w[kCGWindowOwnerPID as String] as? Int) == Int(b.pid) && (w[kCGWindowName as String] as? String) == b.title
            && (w[kCGWindowBounds as String] as? NSDictionary).flatMap { CGRect(dictionaryRepresentation: $0 as CFDictionary) } == b.rect
    }
    let ax = ((axAttr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? []).filter {
        sameFrame($0, b.rect) && (b.title == nil || (axAttr($0, kAXTitleAttribute) as? String) == b.title)
    }
    guard cg.count == 1, ax.count == 1 else { throw InputError(reason: "needs_front") }
    return ax[0]
}

/// postToPid の key は app の key の窓に届き、映している窓とは限らない（W16M-2 で実測: TextEdit の後から開いた別の書類に入った）。
/// 送る前に、映している窓（`mirroredWindow`）を AX で main にし（activate しない・前面と他の app の窓の上下は変わらない＝実測）、app の focused の
/// 窓がその窓そのもの（CFEqual・位置と大きさが同じ別の窓では足りない）になった事を確かめる。なれなければ送らない（owner の別の書類に打たない）
func focusMirroredWindow(_ b: Bounds) throws {
    guard AXIsProcessTrusted() else { throw InputError(reason: "not_trusted") }
    let app = AXUIElementCreateApplication(b.pid)
    AXUIElementSetMessagingTimeout(app, 5)
    let w = try mirroredWindow(app, b)
    // app の focused の窓がこの窓か、app が focused の窓を答えない時（背景で起きる・実測）はこの窓が main か
    let focused = { () -> Bool in
        guard let f = axAttr(app, kAXFocusedWindowAttribute), CFGetTypeID(f) == AXUIElementGetTypeID() else { return (axAttr(w, kAXMainAttribute) as? Bool) == true }
        return CFEqual(f, w)
    }
    if focused() { return }
    AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue)
    for _ in 0..<25 {
        if focused() { return }
        usleep(20_000)
    }
    throw InputError(reason: "needs_front")
}

/// 1 回の click を AX で: その点の要素を app の要素から引き（他の app の窓に隠れていても・前に出さない）、押すか、文字の欄なら選択の位置を置く
func axAtPoint(_ cmd: [String: Any], _ b: Bounds) throws -> [String: Any] {
    guard let x = (cmd["x"] as? NSNumber)?.doubleValue, let y = (cmd["y"] as? NSNumber)?.doubleValue, (0...1).contains(x), (0...1).contains(y)
    else { throw InputError(reason: "bad_point") }
    // 2 回以上の click（語・行の選択）は AX に同じ物が無い
    guard ((cmd["count"] as? Int) ?? 1) == 1 else { throw InputError(reason: "needs_front") }
    let p = CGPoint(x: b.rect.minX + x * b.rect.width, y: b.rect.minY + y * b.rect.height)
    let app = AXUIElementCreateApplication(b.pid)
    AXUIElementSetMessagingTimeout(app, 5)
    let w = try mirroredWindow(app, b)
    // その点で同じ app の別の窓（同じ位置と大きさの双子を含む）が前に在れば、その窓の要素が返る → 押さない
    var hit: AXUIElement?
    guard AXUIElementCopyElementAtPosition(app, Float(p.x), Float(p.y), &hit) == .success, let e = hit, let hw = windowOf(e), CFEqual(hw, w)
    else { throw InputError(reason: "needs_front") }
    let role = (axAttr(e, kAXRoleAttribute) as? String) ?? ""
    var names: CFArray?
    if AXUIElementCopyActionNames(e, &names) == .success, ((names as? [String]) ?? []).contains(kAXPressAction) {
        let r = AXUIElementPerformAction(e, kAXPressAction as CFString)
        guard r == .success else { throw InputError(reason: "ax_press_failed_\(r.rawValue)") }
        return ["ax": "press", "role": role, "point": ["x": p.x, "y": p.y]]
    }
    var settable = DarwinBoolean(false)
    guard AXUIElementIsAttributeSettable(e, kAXSelectedTextRangeAttribute as CFString, &settable) == .success, settable.boolValue
    else { throw InputError(reason: "needs_front") }
    var pt = p, out: CFTypeRef?, at = CFRange()
    guard let pv = AXValueCreate(.cgPoint, &pt),
          AXUIElementCopyParameterizedAttributeValue(e, kAXRangeForPositionParameterizedAttribute as CFString, pv, &out) == .success,
          let v = out, CFGetTypeID(v) == AXValueGetTypeID(), AXValueGetValue(v as! AXValue, .cfRange, &at)
    else { throw InputError(reason: "needs_front") }
    // 最後の行より下の点は AXRangeForPosition が 0 を返す（TextEdit で実測）。本物の click と同じく末尾に置く
    if let n = (axAttr(e, kAXNumberOfCharactersAttribute) as? NSNumber)?.intValue, n > 0, let last = rangeBounds(e, n - 1, 1), p.y > last.maxY {
        at.location = n
    }
    var caret = CFRange(location: at.location, length: 0)
    guard let cv = AXValueCreate(.cfRange, &caret) else { throw InputError(reason: "event_create_failed") }
    let r = AXUIElementSetAttributeValue(e, kAXSelectedTextRangeAttribute as CFString, cv)
    guard r == .success else { throw InputError(reason: "ax_select_failed_\(r.rawValue)") }
    var out2: [String: Any] = ["ax": "select", "role": role, "loc": at.location, "point": ["x": p.x, "y": p.y]]
    // W16M-3（不具合 ②）: 背景の NSTextView は insertion point を描かない（映像に caret が出ず「cursor が動かない」に見えた）。
    // 選択を読み戻した位置の caret の矩形（窓の中の割合・文字の左の辺）を答えに付け、Shibubu が映像の上に描く。読めなければ付けない（描かない）
    if let sv = axAttr(e, kAXSelectedTextRangeAttribute), CFGetTypeID(sv) == AXValueGetTypeID() {
        var sel = CFRange()
        if AXValueGetValue(sv as! AXValue, .cfRange, &sel), let c = caretRect(e, sel.location), b.rect.width > 0, b.rect.height > 0 {
            out2["loc"] = sel.location
            out2["caret"] = ["x": (c.minX - b.rect.minX) / b.rect.width, "y": (c.minY - b.rect.minY) / b.rect.height,
                             "w": 0, "h": c.height / b.rect.height]
        }
    }
    return out2
}

/// 文字の欄 e の loc の前の caret の矩形（画面の point・幅 0）。AXBoundsForRange の長さ 0 は画面の下の端の 0×0 を返す（TextEdit で実測）ので
/// 長さ 1 の左の辺。末尾（loc = 文字数）は最後の文字の右の辺・最後が改行なら次の行の頭（左の端は 0 文字目の矩形の左）
func caretRect(_ e: AXUIElement, _ loc: Int) -> CGRect? {
    let n = (axAttr(e, kAXNumberOfCharactersAttribute) as? NSNumber)?.intValue ?? 0
    if loc < n, let r = rangeBounds(e, loc, 1) { return CGRect(x: r.minX, y: r.minY, width: 0, height: r.height) }
    guard n > 0, loc == n, let last = rangeBounds(e, n - 1, 1) else { return nil }
    var range = CFRange(location: n - 1, length: 1), out: CFTypeRef?
    let newline = AXValueCreate(.cfRange, &range).flatMap { rv in
        AXUIElementCopyParameterizedAttributeValue(e, kAXStringForRangeParameterizedAttribute as CFString, rv, &out) == .success ? out as? String : nil
    } == "\n"
    if newline, let first = rangeBounds(e, 0, 1) { return CGRect(x: first.minX, y: last.maxY, width: 0, height: last.height) }
    return CGRect(x: last.maxX, y: last.minY, width: 0, height: last.height)
}

/// 4 文字の Apple Event の code（'core'・'save'・'cwin' など）
func fcc(_ s: String) -> FourCharCode { s.utf8.reduce(0) { $0 << 8 | FourCharCode($1) } }

/// Apple Event の object specifier（want / form / seld / from）
func objectSpecifier(want: String, form: String, data: NSAppleEventDescriptor, from: NSAppleEventDescriptor) -> NSAppleEventDescriptor {
    let s = NSAppleEventDescriptor.record().coerce(toDescriptorType: fcc("obj "))!
    s.setDescriptor(NSAppleEventDescriptor(typeCode: fcc(want)), forKeyword: fcc("want"))
    s.setDescriptor(NSAppleEventDescriptor(enumCode: fcc(form)), forKeyword: fcc("form"))
    s.setDescriptor(data, forKeyword: fcc("seld"))
    s.setDescriptor(from, forKeyword: fcc("from"))
    return s
}

/// ⌘S: 映している窓の書類（`document of window id <窓 id>`・Cocoa scripting の窓の id は CGWindowID と同じ＝実測）を、その pid の app に
/// Apple Event の save（'core' / 'save'）で保存させる。宛先は pid で決める（同じ bundle の app が 2 つ起きていると、bundle id 宛ては別の process に
/// 届いた＝W16M-2 で実測）。書類は窓で決める（題名で探さない＝同じ名前の別の書類を保存しない）。neverInteract（保存の panel などを出させない）。
/// 許可（オートメーション）の無い時は -1743 → automation_denied。それ以外の失敗（save を持たない app・書類の無い窓・時間切れ）は needs_front。app は前に出さない。
/// 但し許可がまだ決まっていない時（-1744）は、neverInteract でも macOS が「Shibubu に <app> の操作を許可しますか」を出す（前に出る・M4 ③a の .app で実測）。
/// 答えは待たずに -1744 → needs_front で返る（利用者が許可すると次の ⌘S から通る）
func saveDocument(window: CGWindowID, pid: pid_t) throws -> [String: Any] {
    let win = objectSpecifier(want: "cwin", form: "ID  ", data: NSAppleEventDescriptor(int32: Int32(bitPattern: window)), from: .null())
    let doc = objectSpecifier(want: "prop", form: "prop", data: NSAppleEventDescriptor(typeCode: fcc("docu")), from: win)
    let ev = NSAppleEventDescriptor(eventClass: fcc("core"), eventID: fcc("save"), targetDescriptor: NSAppleEventDescriptor(processIdentifier: pid),
                                    returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
    ev.setParam(doc, forKeyword: fcc("----"))
    let code: Int
    do {
        let reply = try ev.sendEvent(options: [.waitForReply, .neverInteract], timeout: 10)
        code = Int(reply.paramDescriptor(forKeyword: fcc("errn"))?.int32Value ?? 0)
    } catch let e as NSError {
        code = e.code
    }
    if code == 0 { return ["saved": "document of window \(window)"] }
    throw InputError(reason: code == -1743 ? "automation_denied" : "needs_front")
}

/// W16M-3（窓の選び方）: 窓の書類の path（`path of document of window id <窓 id>`・NSDocument の fileName = 'ppth'）を、その pid の app に Apple Event の get で問う。
/// 窓は saveDocument と同じく窓 id で決める（題名で探さない＝同じ題名の別の書類と取り違えない・背景で AX が答えない app でも答える）。
/// オートメーションの許可がまだ無い時は問わない（AEDeterminePermissionToAutomateTarget の askUserIfNeeded = false＝許可を問う画面を出さない・前に何も出さない）。
/// ask = true（`--ask`・利用者が Shibubu の button を押した時だけ・M4 ③）の時だけ、まだ決まっていない許可を macOS に尋ねさせる（確認の画面が前に出る）。
/// 返す: {"path": "/abs"}・引けない時は {"path": null, "reason": "not_permitted_<code>" | "no_path" | "ae_<code>"}（名前の無い書類・scripting の無い app）
func documentPath(window: CGWindowID, pid: pid_t, ask: Bool) -> [String: Any] {
    let target = NSAppleEventDescriptor(processIdentifier: pid)
    let perm = AEDeterminePermissionToAutomateTarget(target.aeDesc, AEEventClass(typeWildCard), AEEventID(typeWildCard), ask)
    guard perm == noErr else { return ["path": NSNull(), "reason": "not_permitted_\(perm)"] }
    let win = objectSpecifier(want: "cwin", form: "ID  ", data: NSAppleEventDescriptor(int32: Int32(bitPattern: window)), from: .null())
    let doc = objectSpecifier(want: "prop", form: "prop", data: NSAppleEventDescriptor(typeCode: fcc("docu")), from: win)
    let path = objectSpecifier(want: "prop", form: "prop", data: NSAppleEventDescriptor(typeCode: fcc("ppth")), from: doc)
    let ev = NSAppleEventDescriptor(eventClass: fcc("core"), eventID: fcc("getd"), targetDescriptor: target,
                                    returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
    ev.setParam(path, forKeyword: fcc("----"))
    do {
        let reply = try ev.sendEvent(options: [.waitForReply, .neverInteract], timeout: 5)
        if let e = reply.paramDescriptor(forKeyword: fcc("errn"))?.int32Value, e != 0 { return ["path": NSNull(), "reason": "ae_\(e)"] }
        guard let s = reply.paramDescriptor(forKeyword: fcc("----"))?.stringValue, s.hasPrefix("/") else { return ["path": NSNull(), "reason": "no_path"] }
        return ["path": s]
    } catch let e as NSError {
        return ["path": NSNull(), "reason": "ae_\(e.code)"]
    }
}

// ============================== Accessibility（scroll の意味の読み戻し） ==============================

func axAttr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: CFTypeRef?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}

/// 題名の合う窓（無ければ focused・最初の窓 = `axWindow`）の中で最初に見つかる縦の scroll bar の値（深さ 8 まで）。
/// 題名で引く（M4: 書類なしで起こした TextEdit は題名の無い窓も持ち、focused / 最初の窓がそちらだと null を返していた）
func verticalScroll(pid: pid_t, title: String?) -> Double? {
    guard let win = axWindow(pid: pid, title: title) else { return nil }
    var queue: [(AXUIElement, Int)] = [(win, 0)]
    while !queue.isEmpty {
        let (e, depth) = queue.removeFirst()
        if (axAttr(e, kAXRoleAttribute) as? String) == kAXScrollBarRole,
           (axAttr(e, kAXOrientationAttribute) as? String) == kAXVerticalOrientationValue,
           let v = axAttr(e, kAXValueAttribute) as? NSNumber { return v.doubleValue }
        if depth < 8, let kids = axAttr(e, kAXChildrenAttribute) as? [AXUIElement] { queue += kids.map { ($0, depth + 1) } }
    }
    return nil
}

/// 題名が合う窓（無ければ focused・最初の窓）。AX は CGWindowID を公開 API で出さないので、題名で突き合わせる
func axWindow(pid: pid_t, title: String?) -> AXUIElement? {
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 10)
    let wins = (axAttr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? []
    if let t = title, let w = wins.first(where: { (axAttr($0, kAXTitleAttribute) as? String) == t }) { return w }
    if let f = axAttr(app, kAXFocusedWindowAttribute) { return (f as! AXUIElement) }
    return wins.first
}

func firstTextArea(_ win: AXUIElement) -> AXUIElement? {
    var queue: [(AXUIElement, Int)] = [(win, 0)]
    while !queue.isEmpty {
        let (e, depth) = queue.removeFirst()
        if (axAttr(e, kAXRoleAttribute) as? String) == kAXTextAreaRole { return e }
        if depth < 8, let kids = axAttr(e, kAXChildrenAttribute) as? [AXUIElement] { queue += kids.map { ($0, depth + 1) } }
    }
    return nil
}

/// 窓を main にして文書を focused にする。"ok" か失敗した段の名前
func axFocus(pid: pid_t, title: String?) -> String? {
    guard let w = axWindow(pid: pid, title: title) else { return "ax_no_window" }
    let main = AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue)
    guard let t = firstTextArea(w) else { return "ax_no_text_area(main=\(main.rawValue))" }
    let focused = AXUIElementSetAttributeValue(t, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    return main == .success && focused == .success ? "ok" : "ax_set_failed(main=\(main.rawValue),focused=\(focused.rawValue))"
}

/// 文字の欄 e の loc から len 文字の矩形（画面の point・top-left 原点）
func rangeBounds(_ e: AXUIElement, _ loc: Int, _ len: Int) -> CGRect? {
    var range = CFRange(location: loc, length: len)
    guard let r = AXValueCreate(.cfRange, &range) else { return nil }
    var out: CFTypeRef?
    guard AXUIElementCopyParameterizedAttributeValue(e, kAXBoundsForRangeParameterizedAttribute as CFString, r, &out) == .success,
          let v = out, CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
    var rect = CGRect.zero
    return AXValueGetValue(v as! AXValue, .cgRect, &rect) ? rect : nil
}

/// 前の窓の最初の AXTextArea で、文字の範囲の矩形（画面の point・top-left 原点）
func textBounds(pid: pid_t, loc: Int, len: Int, title: String? = nil) -> CGRect? {
    guard let win = axWindow(pid: pid, title: title), let t = firstTextArea(win) else { return nil }
    return rangeBounds(t, loc, len)
}

// ============================== stream ==============================

final class Mirror: NSObject, SCStreamOutput, SCStreamDelegate {
    let window: CGWindowID
    let discard: Bool
    /// G32: stream の番号（1 = --window の窓・2 から = stdin の open）。frame の header に付ける
    let id: Int
    /// G32: 途中で止まった時（-3805・許可を外された）。表から外して stopped を 1 回出す（`startStream` が置く）
    var onStop: (String, String) -> Void = { _, _ in }
    var seq = 0
    var bytes = 0
    var dst: UnsafeMutableRawPointer? = nil
    var dstSize = 0

    init(window: CGWindowID, discard: Bool, id: Int) { self.window = window; self.discard = discard; self.id = id }

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen,
              let atts = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let info = atts.first,
              let raw = info[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
              let px = CMSampleBufferGetImageBuffer(sb) else { return }
        let display = (info[.displayTime] as? UInt64).map(machToMs) ?? nowMs()
        var content = CGRect.zero
        if let d = info[.contentRect] as? NSDictionary, let r = CGRect(dictionaryRepresentation: d as CFDictionary) { content = r }
        let scale = (info[.contentScale] as? CGFloat) ?? 1
        let factor = (info[.scaleFactor] as? CGFloat) ?? 1
        let w = CVPixelBufferGetWidth(px), h = CVPixelBufferGetHeight(px)
        seq += 1
        let header: [String: Any] = [
            "seq": seq, "w": w, "h": h, "captured_ms": display, "sent_ms": nowMs(), "stream": id,
            // contentRect は窓の point の座標・contentScale は窓 → frame の縮尺・scaleFactor は画面の倍率（Retina = 2）
            "content": ["x": content.origin.x, "y": content.origin.y, "w": content.width, "h": content.height],
            "scale": scale, "factor": factor,
        ]
        if discard { bytes += w * h * 4; return }
        // MIRROR-SPEED-1101: IOSurface を写さずに Shibubu へ（--surfaces）。受け口が満ちている = この 1 枚を捨てる・受け口が無い = 下の pipe（以後ずっと）
        if let io = CVPixelBufferGetIOSurface(px)?.takeUnretainedValue() {
            switch surfaces.send(io, json(header)) {
            case .sent, .dropped: return
            case .off: break
            }
        }
        CVPixelBufferLockBaseAddress(px, .readOnly); defer { CVPixelBufferUnlockBaseAddress(px, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(px) else { return }
        // BGRA → RGBA（egui の形）。行の余白（bytesPerRow）もここで詰める
        let need = w * h * 4
        if dstSize < need { dst?.deallocate(); dst = UnsafeMutableRawPointer.allocate(byteCount: need, alignment: 16); dstSize = need }
        var s = vImage_Buffer(data: base, height: vImagePixelCount(h), width: vImagePixelCount(w), rowBytes: CVPixelBufferGetBytesPerRow(px))
        var d = vImage_Buffer(data: dst, height: vImagePixelCount(h), width: vImagePixelCount(w), rowBytes: w * 4)
        let map: [UInt8] = [2, 1, 0, 3]
        vImagePermuteChannels_ARGB8888(&s, &d, map, vImage_Flags(kvImageNoFlags))
        let hd = json(header)
        var payload = le32(hd.count); payload.append(hd); payload.append(Data(bytesNoCopy: dst!, count: need, deallocator: .none))
        bytes += need
        message(1, payload)
    }

    // r10（36 回目・macOS 14.5）: ScreenCaptureKit が nonnull の error に nil を渡して呼ぶ事がある。型が `Error` のままだと @objc の thunk が nil を
    // そのまま渡し、下の `"\(error)"`（`_print_unlocked`）が 0 番地を読んで helper が SIGSEGV で落ちた（crash report: didStopWithError + 824 の直前の
    // call = `_print_unlocked`・rdi = 0）。Optional で受ける = thunk が nil を確かめる（swiftc は「protocol と optionality が違う」の warning を 1 つ出す）
    func stream(_ stream: SCStream, didStopWithError error: Error?) {
        // W16M-3: stream の途中で画面収録の許可を外された時は Rust の ScreenCaptureDenied（「許可が要ります」の 1 行）に読まれる reason で。
        // -3801（declined TCCs）は load 400 の下で許可のあるまま出た（docs/evidence/W16M-3/click-diagnosis.txt）ので error の code では決めない。
        // G32: 元の app が終わると 2 秒の見張りより先にここへ -3815 で来る（実測）。窓がもう無ければ window_gone（「元の窓が閉じました」の 1 行）・在って隠れていれば window_hidden
        let reason = bounds(window) == nil ? goneReason(window) : CGPreflightScreenCaptureAccess() ? "stream_error" : "no_screen_capture_permission"
        onStop(reason, error.map { "\($0)" } ?? "ScreenCaptureKit stopped the stream without an error")
    }
}

func arg(_ args: [String], _ name: String) -> String? {
    guard let i = args.firstIndex(of: name), i + 1 < args.count else { return nil }
    return args[i + 1]
}

/// MIRROR-QUIET-1101: 画面の倍率（主の display の px / point）。AppKit の NSScreen を使わない＝NSApplication を作らない・読めなければ 2
func backingScale() -> CGFloat {
    guard let m = CGDisplayCopyDisplayMode(CGMainDisplayID()), m.width > 0 else { return 2 }
    return CGFloat(m.pixelWidth) / CGFloat(m.width)
}

/// MIRROR-SPEED-1101: 撮る大きさ（pixel）= 窓の pixel（point × 画面の倍率）を、Shibubu が描く大きさ（`view`・pixel）と --max-width に収まるまで縮めた物
/// （縦横の比は窓のまま・窓より細かくは撮らない）。view が nil = まだ聞いていない = 窓の pixel のまま
func captureSize(_ size: CGSize, view: CGSize?, maxWidth: Int) -> (Int, Int) {
    let backing = backingScale()
    let px = CGSize(width: size.width * backing, height: size.height * backing)
    var k = min(1, CGFloat(maxWidth) / max(1, px.width))
    if let v = view, v.width >= 2, v.height >= 2 { k = min(k, v.width / max(1, px.width), v.height / max(1, px.height)) }
    return (max(2, Int((px.width * k).rounded())), max(2, Int((px.height * k).rounded())))
}

/// MIRROR-SPEED-1101（owner 205）: 撮る枚数（--smooth と stdin の {"cmd":"smoothness"}・Settings › General の Mirror smoothness）。smooth = 60・saver = 15・
/// auto = 60 で始め、機械が混んだら（load の 1 分平均 ÷ CPU の数）30・15 に下げる（遅れより先に枚数を落とす）。下げる閾値と戻す閾値を分ける（ばたつかない）
final class Pace: @unchecked Sendable {
    private let lock = NSLock()
    private var mode = "auto", tier: Int32 = 60
    /// 知らない語は false（呼ぶ側が bad_smoothness と言う）
    func set(_ m: String) -> Bool {
        guard ["auto", "smooth", "saver"].contains(m) else { return false }
        lock.lock(); defer { lock.unlock() }
        mode = m
        return true
    }
    /// 今の枚数（2 秒の見張りが毎回呼ぶ = auto の段はここで動く）
    func fps() -> Int32 {
        lock.lock(); defer { lock.unlock() }
        switch mode {
        case "smooth": return 60
        case "saver": return 15
        default:
            var load = [Double](repeating: 0, count: 3)
            guard getloadavg(&load, 3) > 0 else { return tier }
            let busy = load[0] / Double(max(1, ProcessInfo.processInfo.activeProcessorCount))
            // ponytail: load の 1 分平均は遅れて動く（混み始めて数十秒は 60 のまま・空いて数十秒は低いまま）。早く合わせたい時は
            // Shibubu の「撮った → 描いた」の 95% 点（app.log の 1 分の行）を helper へ返して段を決める
            if busy > 3 { tier = 15 } else if busy > 1.5 { tier = min(tier, 30) } else if busy < 1 { tier = 60 }
            if tier == 15 && busy < 2 { tier = 30 }
            return tier
        }
    }
}

let pace = Pace()

/// MIRROR-SPEED-1101（owner 205）: Shibubu の IOSurface の受け口（--surfaces NAME・頭の mach message の並び）。引けない・死んだら off（pipe の tag 1 に戻る）
final class Surfaces: @unchecked Sendable {
    enum Sent { case sent, dropped, off }
    private let lock = NSLock()
    private var server = mach_port_t(MACH_PORT_NULL)
    /// 受け口が在る（SCK の pool を 5 枚にする）
    var on: Bool { lock.lock(); defer { lock.unlock() }; return server != mach_port_t(MACH_PORT_NULL) }
    func connect(_ name: String?) {
        guard let name else { return }
        var port = mach_port_t(MACH_PORT_NULL)
        let kr = bootstrap_look_up(bootstrap_port, name, &port)
        guard kr == KERN_SUCCESS else { log("MIRROR-SPEED-1101: Shibubu の受け口 \(name) を引けません（\(kr)）。画素は pipe で送ります"); return }
        lock.lock(); server = port; lock.unlock()
    }
    /// 1 frame を 1 通で（IOSurface の port を MOVE_SEND で渡し、header JSON を後ろに）。待たない
    func send(_ io: IOSurfaceRef, _ header: Data) -> Sent {
        lock.lock(); let server = self.server; lock.unlock()
        guard server != mach_port_t(MACH_PORT_NULL) else { return .off }
        let port = IOSurfaceCreateMachPort(io)
        guard port != mach_port_t(MACH_PORT_NULL) else { return .off }
        let size = 44 + (header.count + 3) / 4 * 4
        let buf = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: 8); defer { buf.deallocate() }
        buf.initializeMemory(as: UInt8.self, repeating: 0, count: size)
        // header（bits = COMPLEX | COPY_SEND・size・remote・id 0x5348）・記述子の数 1・port 記述子（name・pad・MOVE_SEND 17 << 16 | type 0）・[len][JSON]
        let words: [(Int, UInt32)] = [(0, 0x8000_0013), (4, UInt32(size)), (8, server), (20, 0x5348), (24, 1), (28, port), (36, 17 << 16), (40, UInt32(header.count))]
        for (at, v) in words { buf.storeBytes(of: v, toByteOffset: at, as: UInt32.self) }
        header.copyBytes(to: (buf + 44).assumingMemoryBound(to: UInt8.self), count: header.count)
        // MACH_SEND_MSG | MACH_SEND_TIMEOUT（0x11）・待たない（0 ms）
        let r = mach_msg(buf.assumingMemoryBound(to: mach_msg_header_t.self), 0x11, mach_msg_size_t(size), 0, mach_port_name_t(MACH_PORT_NULL), 0, mach_port_name_t(MACH_PORT_NULL))
        if r == KERN_SUCCESS { return .sent }
        mach_port_deallocate(mach_task_self_, port)
        // MACH_SEND_TIMED_OUT（0x1000_0004）= 受け口が満ちている（Shibubu が読み切れていない）→ この 1 枚を捨てる。他（受け口が無い）→ 以後 pipe
        if r == 0x1000_0004 { return .dropped }
        lock.lock(); self.server = mach_port_t(MACH_PORT_NULL); lock.unlock()
        log("MIRROR-SPEED-1101: IOSurface を送れません（\(r)）。以後は pipe で送ります")
        return .off
    }
}

let surfaces = Surfaces()

/// G32: stream 1 本（窓と pid は対・入力はこの stream の順の queue で送る＝他の stream の入力を待たせない）
final class Live: @unchecked Sendable {
    let id: Int, window: CGWindowID, pid: pid_t, stream: SCStream, mirror: Mirror, frames: DispatchQueue
    let inputs: DispatchQueue
    /// G91: stream を作った時の設定（大きさと枚数を替える時に同じ物を updateConfiguration に渡す）。MIRROR-SPEED-1101: --fps を渡した stream は Pace で替えない
    let cfg: SCStreamConfiguration, maxWidth: Int, fixedFps: Bool
    private let lock = NSLock()
    /// G91: 窓の大きさ（point）。2 秒の見張りが窓の大きさの替わりを見て撮る大きさを合わせ直す
    private var size: CGSize
    /// MIRROR-SPEED-1101: Shibubu が描く大きさ（pixel・{"cmd":"view"}）。nil = まだ聞いていない = 窓の pixel で撮る
    private var drawn: CGSize? = nil
    private var rate: Int32
    init(id: Int, window: CGWindowID, pid: pid_t, stream: SCStream, mirror: Mirror, frames: DispatchQueue, cfg: SCStreamConfiguration, maxWidth: Int, size: CGSize,
         fps: Int32, fixedFps: Bool) {
        (self.id, self.window, self.pid, self.stream, self.mirror, self.frames) = (id, window, pid, stream, mirror, frames)
        (self.cfg, self.maxWidth, self.size, self.rate, self.fixedFps) = (cfg, maxWidth, size, fps, fixedFps)
        inputs = DispatchQueue(label: "kangae.mirror.input.\(id)")
    }
    /// 今の窓の大きさ（point）と撮る枚数（2 秒の見張りが読む）
    var now: (size: CGSize, fps: Int32) { lock.lock(); defer { lock.unlock() }; return (size, rate) }
    /// G91: 窓の大きさが替わった（Shibubu が本物を枠の大きさにした・人が大きさを変えた）→ 撮る大きさを合わせる（縦横比が替わっても映像を歪めない）
    func resize(_ s: CGSize) { lock.lock(); defer { lock.unlock() }; size = s; fit() }
    /// MIRROR-SPEED-1101: Shibubu が描く大きさが替わった（Page の枠・拡大・四隅）
    func view(_ v: CGSize) { lock.lock(); defer { lock.unlock() }; drawn = v; fit() }
    /// MIRROR-SPEED-1101: 撮る枚数（Pace・2 秒の見張りと stdin の smoothness）。--fps の stream・同じ枚数なら何もしない
    func pace(_ fps: Int32) {
        lock.lock(); defer { lock.unlock() }
        guard !fixedFps, fps != rate else { return }
        rate = fps
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: fps)
        update("枚数")
    }
    /// lock の中で: 撮る大きさ = 窓の pixel と描く大きさの小さい方（captureSize）。替わった時だけ組み直す
    private func fit() {
        let (w, h) = captureSize(size, view: drawn, maxWidth: maxWidth)
        guard w != cfg.width || h != cfg.height else { return }
        cfg.width = w; cfg.height = h
        update("大きさ")
    }
    private func update(_ what: String) {
        stream.updateConfiguration(cfg) { error in if let error { log("G91: stream \(self.id) の\(what)を合わせられません: \(error)") } }
    }
}

/// G32: 動いている stream の表（stdin の thread・起こす Task・2 秒の見張り・SCK の delegate が触る）。表から外した側が stopped を 1 回出す
final class Streams: @unchecked Sendable {
    private let lock = NSLock()
    private var live: [Int: Live] = [:]
    func put(_ l: Live) { lock.lock(); defer { lock.unlock() }; live[l.id] = l }
    func get(_ id: Int) -> Live? { lock.lock(); defer { lock.unlock() }; return live[id] }
    func take(_ id: Int) -> Live? { lock.lock(); defer { lock.unlock() }; return live.removeValue(forKey: id) }
    func all() -> [Live] { lock.lock(); defer { lock.unlock() }; return live.values.sorted { $0.id < $1.id } }
    func takeAll() -> [Live] { lock.lock(); defer { lock.unlock() }; let v = Array(live.values); live = [:]; return v }
}

let streams = Streams()

/// stream 1 本の出来事（"stream" を付ける）
func streamEvent(_ id: Int, _ v: [String: Any]) { var v = v; v["stream"] = id; event(v) }

/// --max-width（既定 上限無し）・--fps（無し = Pace の枚数・MIRROR-SPEED-1101）・--discard
struct StreamConfig { let maxWidth: Int; let fps: Int32?; let discard: Bool }

/// startCapture の答え（frame の方が先に来る事がある）
final class StartState: @unchecked Sendable {
    private let lock = NSLock()
    private var returned = false, failed: String? = nil
    func done(_ error: String?) { lock.lock(); defer { lock.unlock() }; if let error { failed = error } else { returned = true } }
    func read() -> (Bool, String?) { lock.lock(); defer { lock.unlock() }; return (returned, failed) }
}

/// G32: 窓 `window` の stream `id` を起こして表に入れる（--window の窓と stdin の open が同じ物を呼ぶ）。始まれば started を出して true、
/// 始まらなければ stopped を出して false（どちらも "stream": id・stream ごとに 1 回）。SCShareableContent は同じ process の中なので生きている他の stream は落ちない
func startStream(id: Int, window: CGWindowID, _ c: StreamConfig) async -> Bool {
    func stop(_ reason: String, _ detail: Any? = nil) -> Bool {
        var v: [String: Any] = ["kind": "stopped", "reason": reason]
        if let detail { v["detail"] = "\(detail)" }
        streamEvent(id, v); return false
    }
    guard CGPreflightScreenCaptureAccess() else { return stop("no_screen_capture_permission") }
    let content: SCShareableContent
    do { content = try await shareable(all: true) } catch { return stop(error is Timeout ? "capture_timeout" : "shareable_failed", error) }
    guard let win = content.windows.first(where: { $0.windowID == window }) else { return stop("window_gone") }
    guard let start = bounds(window) else { return stop(goneReason(window)) }
    let filter = SCContentFilter(desktopIndependentWindow: win)
    let cfg = SCStreamConfiguration()
    let backing = backingScale()
    // MIRROR-SPEED-1101: 始めは窓の pixel のまま撮る（Shibubu が描く大きさは最初の frame の後の {"cmd":"view"}）・枚数は --fps か Pace
    (cfg.width, cfg.height) = captureSize(win.frame.size, view: nil, maxWidth: c.maxWidth)
    let fps = max(1, c.fps ?? pace.fps())
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: fps)
    cfg.pixelFormat = kCVPixelFormatType_32BGRA
    cfg.showsCursor = false
    // MIRROR-SPEED-1101: --surfaces の時は Shibubu が今と 1 つ前の 2 枚を持つ（use count）ので pool を 5 枚に（SCK の書く面が尽きない）
    cfg.queueDepth = surfaces.on ? 5 : 3
    if #available(macOS 14.0, *) { cfg.ignoreShadowsSingleWindow = true }
    let mirror = Mirror(window: window, discard: c.discard, id: id)
    let stream = SCStream(filter: filter, configuration: cfg, delegate: mirror)
    let q = DispatchQueue(label: "kangae.mirror.frames.\(id)")
    do { try stream.addStreamOutput(mirror, type: .screen, sampleHandlerQueue: q) } catch { return stop("start_failed", error) }
    streams.put(Live(id: id, window: window, pid: start.pid, stream: stream, mirror: mirror, frames: q, cfg: cfg, maxWidth: c.maxWidth, size: win.frame.size,
                     fps: fps, fixedFps: c.fps != nil))
    mirror.onStop = { reason, detail in
        if streams.take(id) != nil { streamEvent(id, ["kind": "stopped", "reason": reason, "detail": detail]) }
    }
    // 混んだ機械では startCapture の完了が frame より何十秒も遅れる（load 900 で実測: frame は流れているのに 30 秒返らない）。
    // 完了か最初の frame の早い方で「始まった」とし、どちらも 30 秒来なければ諦める
    let state = StartState(), startedAt = Date()
    nonisolated(unsafe) let s = stream
    Task {
        do { try await s.startCapture(); state.done(nil) } catch { state.done("\(error)") }
    }
    while true {
        // 始まる前に止まった（delegate が表から外して stopped を出した）
        if streams.get(id) == nil { return false }
        let (returned, failed) = state.read()
        if let failed { return streams.take(id) != nil ? stop("start_failed", failed) : false }
        if returned || q.sync(execute: { mirror.seq }) > 0 { break }
        if Date().timeIntervalSince(startedAt) > 30 {
            guard streams.take(id) != nil else { return false }
            stream.stopCapture { _ in }
            return stop("capture_timeout")
        }
        try? await Task.sleep(nanoseconds: 100_000_000)
    }
    streamEvent(id, ["kind": "started", "window_id": Int(window), "pid": Int(start.pid), "w": cfg.width, "h": cfg.height, "backing": backing,
                     "start_ms": Date().timeIntervalSince(startedAt) * 1000])
    return true
}

/// 窓の一覧（`list` と stdin の `{"cmd":"list"}` が同じ物）。取れない時は "error"（no_screen_capture_permission・capture_timeout・shareable_failed）
func windowList(all: Bool) async -> [String: Any] {
    guard CGPreflightScreenCaptureAccess() else { return ["error": "no_screen_capture_permission"] }
    do {
        let ws = try await shareable(all: all).windows
            .filter { $0.windowLayer == 0 && $0.frame.width >= 80 && $0.frame.height >= 60 && $0.owningApplication != nil }
        return ["windows": ws.map(describe)]
    } catch {
        return ["error": error is Timeout ? "capture_timeout" : "shareable_failed", "detail": "\(error)"]
    }
}

/// MIRROR-FRAME-1101: 窓ごとの小さな絵（頭の契約の stdin の shot）。SCShareableContent は 1 回・1 枚ずつ SCScreenshotManager で幅 `width` px に撮り
/// `dir/<窓 id>.png` に書く。撮れた窓の id を返す（撮れない・消えた窓は飛ばす）。許可が無い・取れない = "error"
func windowShots(_ ids: [UInt32], width: Int, dir: String) async -> [String: Any] {
    guard CGPreflightScreenCaptureAccess() else { return ["error": "no_screen_capture_permission"] }
    let all: [SCWindow]
    do { all = try await shareable(all: false).windows } catch { return ["error": error is Timeout ? "capture_timeout" : "shareable_failed"] }
    var done: [Int] = []
    for id in ids {
        guard let w = all.first(where: { $0.windowID == id }), w.frame.width >= 1 else { continue }
        let cfg = SCStreamConfiguration()
        cfg.width = max(1, width)
        cfg.height = max(1, Int((w.frame.height * Double(width) / w.frame.width).rounded()))
        cfg.showsCursor = false
        cfg.ignoreShadowsSingleWindow = true
        guard let img = try? await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(desktopIndependentWindow: w), configuration: cfg),
              let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: dir).appendingPathComponent("\(id).png") as CFURL, "public.png" as CFString, 1, nil)
        else { continue }
        CGImageDestinationAddImage(dest, img, nil)
        if CGImageDestinationFinalize(dest) { done.append(Int(id)) }
    }
    return ["shots": done]
}

/// stream の stdin の 1 行（頭の契約: "cmd" の行は open / close / list・無い行は入力で "stream" の窓へ・無ければ 1）
func streamLine(_ line: String, _ c: StreamConfig) {
    guard let d = line.data(using: .utf8), let cmd = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else {
        event(["kind": "error", "reason": "bad_json"]); return
    }
    let sid = (cmd["stream"] as? NSNumber)?.intValue
    switch cmd["cmd"] as? String {
    case "open":
        guard let sid, let w = (cmd["window"] as? NSNumber)?.uint32Value, streams.get(sid) == nil else {
            event(["kind": "error", "reason": "bad_open", "stream": cmd["stream"] ?? NSNull()]); return
        }
        Task { _ = await startStream(id: sid, window: w, c) }
    case "close":
        guard let sid, let l = streams.take(sid) else { return }
        l.stream.stopCapture { _ in }
        streamEvent(sid, ["kind": "stopped", "reason": "closed"])
    case "list":
        let req = cmd["id"] ?? NSNull(), all = (cmd["all"] as? Bool) ?? false
        Task { event(await windowList(all: all).merging(["kind": "windows", "id": req]) { $1 }) }
    case "shot":
        let req = cmd["id"] ?? NSNull()
        let ids = (cmd["windows"] as? [NSNumber] ?? []).map(\.uint32Value), width = (cmd["width"] as? NSNumber)?.intValue ?? 112
        guard let dir = cmd["dir"] as? String else { event(["kind": "answer", "id": req, "error": "bad_shot"]); return }
        Task { event(await windowShots(ids, width: width, dir: dir).merging(["kind": "answer", "id": req]) { $1 }) }
    case "status":
        // MIRROR-QUIET-1101: status の subcommand と同じ許可（preflight だけ・dialog を出さない）
        event(["kind": "answer", "id": cmd["id"] ?? NSNull(), "screen_capture": CGPreflightScreenCaptureAccess(), "accessibility": AXIsProcessTrusted(),
               "post_event": CGPreflightPostEventAccess()])
    case "doc-path":
        // MIRROR-QUIET-1101: doc-path の subcommand と同じ答え。Apple Event は最長 5 秒（ask は人が答えるまで）待つので stdin の thread を止めない
        nonisolated(unsafe) let req = cmd["id"] ?? NSNull()
        guard let pid = (cmd["pid"] as? NSNumber)?.int32Value, let w = (cmd["window"] as? NSNumber)?.uint32Value else {
            event(["kind": "answer", "id": req, "path": NSNull(), "reason": "bad_doc_path"]); return
        }
        let ask = (cmd["ask"] as? Bool) ?? false
        DispatchQueue.global().async { event(documentPath(window: w, pid: pid, ask: ask).merging(["kind": "answer", "id": req]) { $1 }) }
    case "view":
        // MIRROR-SPEED-1101: Shibubu が描く大きさ（pixel）。窓の pixel より大きければ窓の pixel のまま（Live.fit）
        guard let sid, let l = streams.get(sid), let w = (cmd["w"] as? NSNumber)?.doubleValue, let h = (cmd["h"] as? NSNumber)?.doubleValue else {
            event(["kind": "error", "reason": "bad_view", "stream": cmd["stream"] ?? NSNull()]); return
        }
        l.view(CGSize(width: w, height: h))
    case "smoothness":
        // MIRROR-SPEED-1101: Settings の Mirror smoothness（全部の stream に・--fps の stream は替えない）
        guard let m = cmd["mode"] as? String, pace.set(m) else { event(["kind": "error", "reason": "bad_smoothness"]); return }
        let fps = pace.fps()
        for l in streams.all() { l.pace(fps) }
    case nil:
        let sid = sid ?? 1
        // 口の検査は窓・許可を見る前（W16M-0）
        if let r = portReject(cmd, human: true) { streamEvent(sid, r); return }
        guard let l = streams.get(sid) else {
            var r: [String: Any] = ["kind": "error", "op": cmd["op"] ?? "", "reason": "no_stream"]
            if let id = cmd["id"] { r["id"] = id }
            streamEvent(sid, r); return
        }
        nonisolated(unsafe) let cmd = cmd
        l.inputs.async { streamEvent(sid, sendReport(cmd, window: l.window, pid: l.pid, human: true)) }
    case let other?:
        event(["kind": "error", "reason": "bad_cmd", "cmd": other])
    }
}

func runStream(_ args: [String]) async {
    // MIRROR-SPEED-1101: 960 の上限と 15 枚の既定をやめる（撮る大きさ = 描く大きさ・枚数 = --smooth）
    let c = StreamConfig(maxWidth: arg(args, "--max-width").flatMap(Int.init) ?? Int.max, fps: arg(args, "--fps").flatMap(Int32.init),
                         discard: args.contains("--discard"))
    if let m = arg(args, "--smooth"), !pace.set(m) { log("MIRROR-SPEED-1101: --smooth \(m) を知りません（auto のまま）") }
    surfaces.connect(arg(args, "--surfaces"))
    // MIRROR-QUIET-1101: stdin の status / doc-path に答える版だと最初に言う（Rust の `Routes::caps`・言わない前の版には 1 回きりの process で問う）
    event(["kind": "ready", "caps": ["list", "status", "doc-path", "open", "shot"]])
    // stream: --window の窓が始まらなければ何も持たないので終わる（stopped は startStream が出した）。serve は窓 0 本で stdin を待つ
    if args.first == "stream" {
        guard let id = arg(args, "--window").flatMap(UInt32.init) else { fail("--window <windowID> が要ります") }
        guard await startStream(id: 1, window: id, c) else { exit(3) }
    }
    // stdin の 1 行 1 JSON。閉じたら全部止めて終わる（Kangae が終わった・最後の面を閉じた）
    Thread.detachNewThread {
        while let line = readLine() { streamLine(line, c) }
        let g = DispatchGroup()
        for l in streams.takeAll() { g.enter(); l.stream.stopCapture { _ in g.leave() } }
        _ = g.wait(timeout: .now() + 3)
        exit(0)
    }
    // 2 秒ごとに: 窓が消えた・隠れた stream を止める（別の窓に付け替えない・process は終わらない）・stream ごとに数を出す
    var lastCpu = cpuMs(), lastAt = nowMs()
    while true {
        try? await Task.sleep(nanoseconds: 2_000_000_000)
        let cpu = cpuMs(), at = nowMs()
        let pct = (cpu - lastCpu) / max(1, at - lastAt) * 100
        lastCpu = cpu; lastAt = at
        // MIRROR-SPEED-1101: auto の枚数は 2 秒ごとに混み具合で（smooth / saver は替わらない）
        let fps = pace.fps()
        for l in streams.all() {
            guard let b = bounds(l.window), b.pid == l.pid else {
                if streams.take(l.id) != nil {
                    l.stream.stopCapture { _ in }
                    streamEvent(l.id, ["kind": "stopped", "reason": bounds(l.window) == nil ? goneReason(l.window) : "window_gone"])
                }
                continue
            }
            let now = l.now
            if abs(b.rect.width - now.size.width) > 1 || abs(b.rect.height - now.size.height) > 1 { l.resize(b.rect.size) }
            l.pace(fps)
            let stats: [String: Any] = ["kind": "stats", "stream": l.id, "frames": l.frames.sync { l.mirror.seq }, "bytes": l.frames.sync { l.mirror.bytes },
                                        "cpu_pct": pct, "fps": Int(l.now.fps), "window": ["x": b.rect.minX, "y": b.rect.minY, "w": b.rect.width, "h": b.rect.height]]
            event(stats)
            if l.mirror.discard { log(String(data: json(stats), encoding: .utf8) ?? "") }
        }
    }
}

// ============================== main ==============================

@main
struct KangaeMirror {
    static func main() async {
        // MIRROR-QUIET-1101（owner 208）: NSApplication を作らない・AppKit も読み込まない。作った時点で launchservicesd に app として check in し、
        // Contents/MacOS では Shibubu の bundle id で「もう 1 つの Shibubu」に見えた（MIRROR-AUTO-1101 A の `.prohibited` は check in の後で効かなかった）
        let args = Array(CommandLine.arguments.dropFirst())
        switch args.first ?? "" {
        case "status":
            print(String(data: json(["screen_capture": CGPreflightScreenCaptureAccess(), "accessibility": AXIsProcessTrusted(),
                                      "post_event": CGPreflightPostEventAccess()]), encoding: .utf8)!)
        case "request":
            print(String(data: json(["screen_capture": CGRequestScreenCaptureAccess()]), encoding: .utf8)!)
        case "list":
            let r = await windowList(all: args.contains("--all"))
            if r["error"] as? String == "no_screen_capture_permission" { print("[]"); log("no_screen_capture_permission"); exit(3) }
            guard let ws = r["windows"] else { fail("窓の一覧を取れません: \(r["detail"] ?? r["error"] ?? "")") }
            print(String(data: json(ws), encoding: .utf8)!)
        case "stream", "serve":
            await runStream(args)
        case "input":
            guard let id = arg(args, "--window").flatMap(UInt32.init), let pid = arg(args, "--pid").flatMap(Int32.init),
                  let body = args.last, let cmd = try? JSONSerialization.jsonObject(with: Data(body.utf8)) as? [String: Any]
            else { fail("input --window ID --pid PID '<JSON>'") }
            let r = portReject(cmd, human: false) ?? sendReport(cmd, window: id, pid: pid)
            // postToPid の event は送った process がすぐ終わると届かない（W16M-2 で実測: 待たない 0 回・300ms 待つと届く）。答える前に待つ
            if r["kind"] as? String == "sent" { usleep(300_000) }
            print(String(data: json(r), encoding: .utf8)!)
        case "doc-path":
            guard let id = arg(args, "--window").flatMap(UInt32.init), let pid = arg(args, "--pid").flatMap(Int32.init) else { fail("doc-path --pid PID --window ID [--ask]") }
            print(String(data: json(documentPath(window: id, pid: pid, ask: args.contains("--ask"))), encoding: .utf8)!)
        case "ax-scroll":
            guard let pid = arg(args, "--pid").flatMap(Int32.init) else { fail("ax-scroll --pid PID [--title 題名]") }
            let v: Any = verticalScroll(pid: pid, title: arg(args, "--title")).map { $0 as Any } ?? NSNull()
            print(String(data: json(["value": v]), encoding: .utf8)!)
        case "ax-text":
            // 文書の今の中身（保存していない編集も含む）。入力が届いたかを、保存の成否と分けて確かめる
            guard let pid = arg(args, "--pid").flatMap(Int32.init) else { fail("ax-text --pid PID [--title 題名]") }
            guard let win = axWindow(pid: pid, title: arg(args, "--title")) else { print(#"{"text":null,"reason":"ax_no_window"}"#); exit(1) }
            guard let t = firstTextArea(win) else { print(#"{"text":null,"reason":"ax_no_text_area"}"#); exit(1) }
            var raw: CFTypeRef?
            let err = AXUIElementCopyAttributeValue(t, kAXValueAttribute as CFString, &raw)
            guard err == .success, let v = raw else { print(String(data: json(["text": NSNull(), "reason": "ax_value_error_\(err.rawValue)"]), encoding: .utf8)!); exit(1) }
            let text: String? = (v as? String) ?? (v as? NSAttributedString)?.string
            print(String(data: json(["text": text.map { $0 as Any } ?? NSNull(), "reason": text == nil ? "value_type_\(CFGetTypeID(v))" : "ok"]), encoding: .utf8)!)
        case "ax-bounds":
            guard let pid = arg(args, "--pid").flatMap(Int32.init), let id = arg(args, "--window").flatMap(UInt32.init),
                  let loc = arg(args, "--loc").flatMap(Int.init), let len = arg(args, "--len").flatMap(Int.init)
            else { fail("ax-bounds --pid PID --window ID --loc L --len N") }
            guard let win = axWindow(pid: pid, title: arg(args, "--title")) else { print(#"{"rect":null,"reason":"ax_no_window"}"#); exit(1) }
            guard firstTextArea(win) != nil else { print(#"{"rect":null,"reason":"ax_no_text_area"}"#); exit(1) }
            guard let r = textBounds(pid: pid, loc: loc, len: len, title: arg(args, "--title")), let b = bounds(id), b.rect.width > 0, b.rect.height > 0 else {
                print(#"{"rect":null,"reason":"ax_bounds_failed"}"#); exit(1)
            }
            print(String(data: json(["rect": ["x": (r.minX - b.rect.minX) / b.rect.width, "y": (r.minY - b.rect.minY) / b.rect.height,
                                              "w": r.width / b.rect.width, "h": r.height / b.rect.height]]), encoding: .utf8)!)
        default:
            fail("使い方: status | request | list | stream --window ID [--max-width N] [--fps F] [--discard] [--surfaces NAME] [--smooth auto|smooth|saver] | serve [--max-width N] [--fps F] [--surfaces NAME] [--smooth M] | input --window ID --pid PID '<JSON>' | doc-path --pid PID --window ID [--ask] | ax-scroll --pid PID [--title T] | ax-text --pid PID | ax-bounds --pid PID --window ID --loc L --len N")
        }
    }
}
