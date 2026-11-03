// axprobe — 1 手ごとの AX の切り分けの probe（YC W12W-1c・試験の側・製品の依存ではない・cua-driver を呼ばない）。
// 呼ぶ側の使い方: bin/axprobe --pid <run.json の pid> --step <手の名前> [--want <AX identifier>] [--timeout-ms 10000]
//   → stdout に shibubu.qa.axprobe/1（lib/Records.swift の AxProbe・record.schema.json の axprobe）を 1 つ。
//   run.sh step が cua-driver の手の直後に呼び、steps/<name>.axprobe.json に書く。tally.py の cause_of が cua の応答と並べて判定する。
//   --want の末尾が `*` なら前方一致（例: `web-retry:*` = surface id を知らない時）・`role=AXWebArea` は role で探す（WKWebView の中）。
// 保証する: kAXWindows・kAXMainWindow・kAXFocusedWindow を別々に読み、各々の AXError の名前と応答の時間を残す（空と失敗を混ぜない）。
//   1 属性の読みは --timeout-ms（既定 10000・AXUIElementSetMessagingTimeout）で切る。want の木の探しは 3 属性で取れた窓から幅優先・
//   上限は 5000 要素か --timeout-ms（届く前に止めた・子を読めない枝があれば complete = false）。CG の窓（off screen も）・Space の種類・混雑の生の数を同じ時刻で。
//   その pid の scheduling priority（pri・App Nap の帯 4 を記録だけ）。読むのは --pid の app の AX だけ（owner の app の AX は読まない）。前面・cursor・Space を変えない（読むだけ・押さない）。
// 保証しない: 操作できる事（判定は tally.py）。Space の種類は CG の窓の bounds と menu bar の窓（layer 24）からの推定で、決まらなければ unknown。
//   --windows --pid <pid>: その app の AXWindows を 1 枚ずつ {title, subrole, movable(AXPosition が設定できる), main, minimized, frame} の JSON 配列で 1 行（rehearse.py の主窓選び = 製品 pick_by_role と同じ役割の規則の入力・読むだけ）。読めない属性は null。
//   --poll --out <jsonl> --stop <file> [--interval-ms 500]: cursor の位置と「種類ごとの最後の入力からの秒」（hidSystemState / combinedSessionState × mouse 系・key・flagsChanged・tablet）を interval ごとに 1 行足し、<file> が出来たら終わる（W15-b68・cursor_source.py が D-g の cursor の赤の出所を分ける入力。読むだけ・許可は要らない）。
//   --idle-only: pid なしで {hid_idle_s, mouse_idle_s, key_idle_s} だけを 1 行（owner_idle.py の入力・W15-b42）。
// exit: 0 書いた / 2 使い方 / 3 Accessibility の許可が無い（それでも trusted=false の記録は書く）
import ApplicationServices
import Foundation
import IOKit

/// bridge（KangaeSurfaceBridge.swift）の axName と同じ綴り（別の binary なので写し）。
func axName(_ e: AXError) -> String {
    switch e {
    case .success: return "success"
    case .failure: return "failure"
    case .illegalArgument: return "illegal_argument"
    case .invalidUIElement: return "invalid_ui_element"
    case .invalidUIElementObserver: return "invalid_ui_element_observer"
    case .cannotComplete: return "cannot_complete"
    case .attributeUnsupported: return "attribute_unsupported"
    case .actionUnsupported: return "action_unsupported"
    case .notificationUnsupported: return "notification_unsupported"
    case .notImplemented: return "not_implemented"
    case .notificationAlreadyRegistered: return "notification_already_registered"
    case .notificationNotRegistered: return "notification_not_registered"
    case .apiDisabled: return "api_disabled"
    case .noValue: return "no_value"
    case .parameterizedAttributeUnsupported: return "parameterized_attribute_unsupported"
    case .notEnoughPrecision: return "not_enough_precision"
    @unknown default: return "ax_\(e.rawValue)"
    }
}

func ms(since t0: CFAbsoluteTime) -> Double { ((CFAbsoluteTimeGetCurrent() - t0) * 10_000).rounded() / 10 }

/// 1 属性を 1 回読む。MainWindow / FocusedWindow は配列でなく要素 1 つで返る。
/// cannot_complete は timeout の 9 割以上待った時だけ timeout（すぐ返る cannot_complete は AX の口が無い・pid が GUI でない = error）。
func attempt(_ app: AXUIElement, _ attr: String, trusted: Bool, timeoutMs: Double) -> (AxAttempt, [AXUIElement]) {
    let t0 = CFAbsoluteTimeGetCurrent()
    var v: CFTypeRef?
    let e = AXUIElementCopyAttributeValue(app, attr as CFString, &v)
    let took = ms(since: t0)
    var got: [AXUIElement] = []
    if e == .success, let v {
        if let a = v as? [AXUIElement] { got = a } else if CFGetTypeID(v) == AXUIElementGetTypeID() { got = [v as! AXUIElement] }
    }
    let outcome: AxOutcome
    switch e {
    case .success: outcome = got.isEmpty ? .empty : .answered
    case .noValue: outcome = .empty
    case .cannotComplete: outcome = took >= timeoutMs * 0.9 ? .timeout : .error
    case .apiDisabled: outcome = .notTrusted
    default: outcome = trusted ? .error : .notTrusted
    }
    return (AxAttempt(outcome: outcome, axCode: axName(e), elapsedMs: took, count: got.count), got)
}

/// want の identifier を窓の部分木から幅優先で探す（読むだけ）。
func walk(_ roots: [AXUIElement], want: String, deadline: CFAbsoluteTime, cap: Int = 5000) -> WantProbe {
    let t0 = CFAbsoluteTimeGetCurrent()
    // `role=AXWebArea` は role で探す（WKWebView の中は Shibubu の identifier を持たない）・末尾 `*` は前方一致・それ以外は identifier の一致
    let (attr, key) = want.hasPrefix("role=") ? (kAXRoleAttribute, String(want.dropFirst(5))) : (kAXIdentifierAttribute, want)
    let prefix = key.hasSuffix("*") ? String(key.dropLast()) : nil
    var queue = roots
    var i = 0
    var broken = false
    while i < queue.count, i < cap, CFAbsoluteTimeGetCurrent() < deadline {
        let el = queue[i]
        i += 1
        var idv: CFTypeRef?
        if AXUIElementCopyAttributeValue(el, attr as CFString, &idv) == .success, let id = idv as? String,
           prefix.map({ id.hasPrefix($0) }) ?? (id == key) {
            return WantProbe(identifier: want, found: true, visited: i, complete: false, elapsedMs: ms(since: t0))
        }
        var kids: CFTypeRef?
        let e = AXUIElementCopyAttributeValue(el, kAXChildrenAttribute as CFString, &kids)
        if e == .success, let a = kids as? [AXUIElement] {
            queue += a
        } else if e != .success && e != .noValue && e != .attributeUnsupported {
            broken = true // 子を読めなかった枝がある = 木を全部は見ていない
        }
    }
    return WantProbe(identifier: want, found: false, visited: i, complete: i >= queue.count && !broken, elapsedMs: ms(since: t0))
}

/// 今の（menu bar の在る display の）Space の種類。menu bar（layer 24・display の幅）が見えている = 普通（全画面の Space では隠れる）・
/// layer 0 の on screen の窓が display と同じ枠 = 全画面・display の高さの窓が左端と右端を埋める = Split View・どれでもなければ unknown（推し量らない）。
/// cua-driver の agent cursor の overlay（layer 0・display と同じ枠・試験の道具の窓）は数えない。
func spaceKind(_ all: [WindowInfo]) -> SpaceKind {
    let d = CGDisplayBounds(CGMainDisplayID())
    let near = { (a: Double, b: CGFloat) in abs(a - Double(b)) <= 1 }
    if all.contains(where: { $0.onscreen && $0.layer == 24 && near($0.bounds.y, d.minY) && near($0.bounds.w, d.width) }) { return .normal }
    let wins = all.filter { $0.onscreen && $0.layer == 0 && $0.bounds.w >= 50 && $0.bounds.h >= 50 && $0.owner != "Cua Driver" }
    if wins.contains(where: { near($0.bounds.x, d.minX) && near($0.bounds.y, d.minY) && near($0.bounds.w, d.width) && near($0.bounds.h, d.height) }) {
        return .fullscreen
    }
    let tall = wins.filter { near($0.bounds.y, d.minY) && near($0.bounds.h, d.height) && $0.bounds.w < Double(d.width) }
    if tall.contains(where: { near($0.bounds.x, d.minX) }) && tall.contains(where: { near($0.bounds.x + $0.bounds.w, d.maxX) }) {
        return .splitView
    }
    return .unknown
}

/// その pid の scheduling priority（`ps -o pri` と同じ pti_priority）。背景で App Nap に入った app は 4 の帯に落ちる（R-G2・W16-2 の実測）。
func taskPri(_ pid: Int32) -> Int? {
    var info = proc_taskinfo()
    let size = Int32(MemoryLayout<proc_taskinfo>.size)
    return proc_pidinfo(pid, PROC_PIDTASKINFO, 0, &info, size) == size ? Int(info.pti_priority) : nil
}

func sysctlInt32(_ name: String) -> Int32? {
    var v: Int32 = 0
    var size = MemoryLayout<Int32>.size
    return sysctlbyname(name, &v, &size, nil, 0) == 0 ? v : nil
}

func pressure() -> Pressure {
    var load = [Double](repeating: 0, count: 3)
    let load1 = getloadavg(&load, 3) > 0 ? (load[0] * 100).rounded() / 100 : -1
    let mp: MemoryPressure
    switch sysctlInt32("kern.memorystatus_vm_pressure_level") {
    case 1: mp = .normal
    case 2: mp = .warn
    case 4: mp = .critical
    default: mp = .unknown
    }
    var sw = xsw_usage()
    var size = MemoryLayout<xsw_usage>.size
    let swap = sysctlbyname("vm.swapusage", &sw, &size, nil, 0) == 0 ? (Double(sw.xsu_used) / 1_048_576).rounded() : nil
    var idle: Double?
    let s = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOHIDSystem"))
    if s != 0 {
        if let n = IORegistryEntryCreateCFProperty(s, "HIDIdleTime" as CFString, kCFAllocatorDefault, 0)?.takeRetainedValue() as? NSNumber {
            idle = (n.doubleValue / 1e8).rounded() / 10
        }
        IOObjectRelease(s)
    }
    return Pressure(load1: load1, ncpu: ProcessInfo.processInfo.activeProcessorCount, memoryPressure: mp, swapUsedMb: swap, hidIdleS: idle,
                    mouseIdleS: eventIdle([.mouseMoved, .leftMouseDown, .rightMouseDown, .scrollWheel]), keyIdleS: eventIdle([.keyDown]))
}

/// kCGEventSourceStateHIDSystemState の「種類ごとの最後の入力からの秒」の最小（0.1 秒単位）。
func eventIdle(_ types: [CGEventType]) -> Double? {
    let v = types.map { CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: $0) }.min()
    return v.map { ($0 * 10).rounded() / 10 }
}

/// `--windows`: AXWindows の窓ごとの役割の属性（読むだけ）。値が取れない属性は null（推測で埋めない）。
func axWindowsJSON(_ pid: Int32) -> String {
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 10)
    var ref: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &ref) == .success, let wins = ref as? [AXUIElement] else { return "[]" }
    func val(_ w: AXUIElement, _ a: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(w, a as CFString, &v) == .success ? v : nil
    }
    let rows: [[String: Any]] = wins.map { w in
        var settable: DarwinBoolean = false
        let movable: Any = AXUIElementIsAttributeSettable(w, kAXPositionAttribute as CFString, &settable) == .success ? settable.boolValue : NSNull()
        var pos = CGPoint.zero, size = CGSize.zero
        let hasFrame = (val(w, kAXPositionAttribute)).map { AXValueGetValue($0 as! AXValue, .cgPoint, &pos) } == true
            && (val(w, kAXSizeAttribute)).map { AXValueGetValue($0 as! AXValue, .cgSize, &size) } == true
        return ["title": val(w, kAXTitleAttribute) as? String ?? NSNull(), "subrole": val(w, kAXSubroleAttribute) as? String ?? NSNull(),
                "movable": movable, "main": (val(w, kAXMainAttribute) as? Bool) ?? NSNull(), "minimized": (val(w, kAXMinimizedAttribute) as? Bool) ?? NSNull(),
                "x": hasFrame ? pos.x : NSNull(), "y": hasFrame ? pos.y : NSNull(), "w": hasFrame ? size.width : NSNull(), "h": hasFrame ? size.height : NSNull()]
    }
    let d = (try? JSONSerialization.data(withJSONObject: rows, options: [.sortedKeys])) ?? Data("[]".utf8)
    return String(decoding: d, as: UTF8.self)
}

/// `--poll`: cursor の位置（CGEvent(source: nil).location）と、source state 2 つ × 種類の「最後の入力からの秒」を 1 行ずつ。許可は要らない（mouse_idle_s と同じ API）。
func pollCursor(out: String, stop: String, intervalMs: Int) -> Never {
    FileManager.default.createFile(atPath: out, contents: nil)
    guard let fh = FileHandle(forWritingAtPath: out) else { exit(ExitCode.badInput.rawValue) }
    let kinds: [(String, CGEventType)] = [("moved", .mouseMoved), ("left", .leftMouseDown), ("right", .rightMouseDown), ("other", .otherMouseDown), ("ldrag", .leftMouseDragged),
                                          ("rdrag", .rightMouseDragged), ("odrag", .otherMouseDragged), ("scroll", .scrollWheel), ("tablet", .tabletPointer), ("key", .keyDown), ("flags", .flagsChanged)]
    func idles(_ s: CGEventSourceStateID) -> String {
        kinds.map { k in
            let v = CGEventSource.secondsSinceLastEventType(s, eventType: k.1)
            return "\"\(k.0)\":\(v.isFinite ? String((v * 10).rounded() / 10) : "null")"
        }.joined(separator: ",")
    }
    while !FileManager.default.fileExists(atPath: stop) {
        let p = CGEvent(source: nil)?.location
        let line = "{\"t\":\(Date().timeIntervalSince1970),\"x\":\(p.map { String(Double($0.x)) } ?? "null"),\"y\":\(p.map { String(Double($0.y)) } ?? "null"),\"hid\":{\(idles(.hidSystemState))},\"comb\":{\(idles(.combinedSessionState))}}\n"
        fh.write(line.data(using: .utf8)!)
        Thread.sleep(forTimeInterval: Double(intervalMs) / 1000)
    }
    exit(0)
}

@main
struct AxProbeMain {
    static func main() {
        do {
            if CommandLine.arguments.contains("--poll") {
                let a = try Args(Array(CommandLine.arguments.dropFirst()), flags: ["--poll"])
                guard let ms = Int(a.value("--interval-ms") ?? "500"), ms >= 50 else { throw GuardError.usage("--interval-ms は 50 以上の整数") }
                pollCursor(out: try a.required("--out"), stop: try a.required("--stop"), intervalMs: ms)
            }
            if CommandLine.arguments.contains("--idle-only") {  // owner_idle.py が読む: pid も AX も要らない・pressure の 3 つの idle だけ
                let p = pressure()
                print("{\"hid_idle_s\":\(p.hidIdleS.map { String($0) } ?? "null"),\"mouse_idle_s\":\(p.mouseIdleS.map { String($0) } ?? "null"),\"key_idle_s\":\(p.keyIdleS.map { String($0) } ?? "null")}")
                exit(0)
            }
            if CommandLine.arguments.contains("--windows") {
                let a = try Args(Array(CommandLine.arguments.dropFirst()), flags: ["--windows"])
                guard let pid = Int32(try a.required("--pid")), pid > 0 else { throw GuardError.usage("--pid は正の整数") }
                print(axWindowsJSON(pid))
                exit(AXIsProcessTrusted() ? 0 : ExitCode.permission.rawValue)
            }
            let a = try Args(Array(CommandLine.arguments.dropFirst()))
            guard let pid = Int32(try a.required("--pid")), pid > 0 else { throw GuardError.usage("--pid は正の整数") }
            let step = try a.required("--step")
            let timeoutMs = Double(a.value("--timeout-ms") ?? "10000") ?? -1
            guard timeoutMs > 0 else { throw GuardError.usage("--timeout-ms は正の数（ms）") }
            let want = a.value("--want").flatMap { $0.isEmpty ? nil : $0 }

            let t = Date().timeIntervalSince1970
            let trusted = AXIsProcessTrusted()
            let app = AXUIElementCreateApplication(pid)
            AXUIElementSetMessagingTimeout(app, Float(timeoutMs / 1000))
            let (w, wl) = attempt(app, kAXWindowsAttribute, trusted: trusted, timeoutMs: timeoutMs)
            let (m, ml) = attempt(app, kAXMainWindowAttribute, trusted: trusted, timeoutMs: timeoutMs)
            let (f, fl) = attempt(app, kAXFocusedWindowAttribute, trusted: trusted, timeoutMs: timeoutMs)
            var roots: [AXUIElement] = []
            for el in wl + ml + fl where !roots.contains(where: { CFEqual($0, el) }) { roots.append(el) }
            let wantProbe = want.map { walk(roots, want: $0, deadline: CFAbsoluteTimeGetCurrent() + timeoutMs / 1000) }
            let all = readWindowList(includeOffscreen: true)
            let probe = AxProbe(schema: .axprobe, t: t, step: step, pid: pid, trusted: trusted,
                                ax: AxAttempts(windows: w, mainWindow: m, focusedWindow: f), want: wantProbe,
                                cg: all.filter { $0.pid == pid }, space: spaceKind(all), pressure: pressure(), pri: taskPri(pid))
            try writeJSON(probe, to: nil)
            exit(trusted ? ExitCode.ok.rawValue : ExitCode.permission.rawValue)
        } catch let e as GuardError {
            if case .usage(let m) = e {
                FileHandle.standardError.write("axprobe: \(m)\n使い方: axprobe --pid <pid> --step <name> [--want <AX identifier>] [--timeout-ms 10000]\n".data(using: .utf8)!)
            }
            exit(e.exitCode.rawValue)
        } catch {
            FileHandle.standardError.write("axprobe: \(error)\n".data(using: .utf8)!)
            exit(ExitCode.badInput.rawValue)
        }
    }
}
