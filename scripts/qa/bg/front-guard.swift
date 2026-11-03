// front-guard — D-g（YC_WAVES §1）の「邪魔しない検査」。guard.sh（前面の ASN を 0.2 秒おきに見るだけ）の後継。
// build: scripts/qa/bg/build.sh（lib/Records.swift と一緒に -parse-as-library で compile・SHIBUBU_SIGN_ID で署名して bin/front-guard へ）。
// 許可: Accessibility（focus・leak）と画面収録（窓の名前）。無ければその検出は「未測」で、0 件の緑にしない。
//
// 呼ぶ側の使い方（run.sh の 1 手）:
//   bin/front-guard watch --out R/events.jsonl --stop R/watch.stop --suspect <shibubu_pid> &   # run の始めに 1 回
//   bin/front-guard snap  --out R/steps/03.before.json [--needle "<打つ文字>"] [--pid <手の対象の pid>…]
//   cua-driver call …（element_token だけ）
//   bin/front-guard snap  --out R/steps/03.after.json  [--needle "<打つ文字>"]
//   bin/front-guard judge --step 03 --before … --after … --events R/events.jsonl --suspect <pid> [--suspect <pid>…] [--owner <pid>] > R/steps/03.verdict.json
//   exit: 0 ok / 1 violation / 6 未測あり（どちらも run を止める）/ 2〜5 道具の失敗（ExitCode）
//   bin/front-guard ax --pid <shibubu_pid> --out R/ax/05-after.json   # W12a: AX の木を読むだけ（AXIdentifier つき・下の axTree）
//
// W2b-AX（AX の窓が常に 1 個在る契約を外から測る。Accessibility の許可を持つこの binary に置く＝新しい TCC の許可を増やさない）:
//   bin/front-guard ax-windows --pid <pid> [--variant head] [--timeout-s 10]                 > R/oracle/03.after.axwindows.json  # 1 回（AxWindowsReading）
//   bin/front-guard ax-sample  --pid <pid> --out R/ax-windows.jsonl --stop R/ax.stop [--variant head] [--interval-ms 1000] &  # run の間ずっと
//   bin/front-guard ax-judge   --samples R/ax-windows.jsonl                                  > R/ax-windows.verdict.json        # 純関数（AxWindowsVerdict）
//   exit: ax-windows / ax-sample 0 ok・3 Accessibility の許可無し / ax-judge 0 ok・1 detached あり・6 unresponsive だけか途中で窓が消えた・4 壊れた行
//
// 保証する: judge は JSON だけを読む純関数（画面に触らない）→ 作った before/after/events で BG のまま負の対照が取れる。
// 保証しない: watch の z 標本の間（既定 200ms）より短い重なりは 3 で見落とす（2 の通知は一瞬でも残る）。
// 道具そのものは前面を取らない: snap / watch / judge は NSApplication を作らない（provoke だけが作る）。

import AppKit
import ApplicationServices
import Foundation

@main
struct FrontGuard {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        do {
            let code = try dispatch(args)
            exit(code.rawValue)
        } catch let e as GuardError {
            FileHandle.standardError.write("front-guard: \(e)\n".data(using: .utf8)!)
            exit(e.exitCode.rawValue)
        } catch {
            FileHandle.standardError.write("front-guard: \(error)\n".data(using: .utf8)!)
            exit(ExitCode.usage.rawValue)
        }
    }

    /// 副命令: snap | watch | judge | provoke。知らない副命令は GuardError.usage。
    static func dispatch(_ args: [String]) throws -> ExitCode {
        guard let sub = args.first else { throw GuardError.usage("snap|watch|judge|provoke|ax|ax-windows|ax-sample|ax-judge") }
        let a = try Args(Array(args.dropFirst()), flags: ["--fg-window"])
        switch sub {
        case "snap":
            try writeJSON(try takeSnapshot(needle: a.value("--needle"), pids: try a.pids("--pid")), to: try a.required("--out"))
            return .ok
        case "watch":
            let ms = Int(a.value("--interval-ms") ?? "200") ?? 0
            guard ms > 0 else { throw GuardError.usage("--interval-ms は正の整数") }
            try watch(out: URL(fileURLWithPath: try a.required("--out")), stop: URL(fileURLWithPath: try a.required("--stop")),
                      suspects: try a.pids("--suspect"), intervalMs: ms)
            return .ok
        case "judge":
            let events = try readEvents(try a.required("--events"))
            let v = judge(step: try a.required("--step"), before: try readJSON(Snapshot.self, try a.required("--before")),
                          after: try readJSON(Snapshot.self, try a.required("--after")), events: events, suspects: try a.pids("--suspect"),
                          owner: try a.pids("--owner").last)
            try writeJSON(v, to: nil)
            return verdictExit(v)
        case "ax":
            guard let pid = try a.pids("--pid").last else { throw GuardError.usage("ax --pid <pid> [--max n] [--out file]") }
            try writeJSON(try axTree(pid: pid, limit: Int(a.value("--max") ?? "3000") ?? 3000), to: a.value("--out"))
            return .ok
        case "provoke":
            guard let d = a.positional.first.flatMap(Detection.init(rawValue:)) else { throw GuardError.usage("provoke <detection> --fg-window") }
            try provoke(d, fgWindow: a.flag("--fg-window"))
            return .ok
        case "ax-windows":
            guard let pid = try a.pids("--pid").first else { throw GuardError.usage("ax-windows --pid <pid>") }
            try writeJSON(try readAxWindows(pid: pid, variant: a.value("--variant"), timeoutS: Double(a.value("--timeout-s") ?? "10") ?? 10), to: nil)
            return .ok
        case "ax-sample":
            guard let pid = try a.pids("--pid").first else { throw GuardError.usage("ax-sample --pid <pid> --out <path> --stop <path>") }
            let ms = Int(a.value("--interval-ms") ?? "1000") ?? 0
            guard ms > 0 else { throw GuardError.usage("--interval-ms は正の整数") }
            try sampleAxWindows(pid: pid, variant: a.value("--variant"), out: URL(fileURLWithPath: try a.required("--out")),
                                stop: URL(fileURLWithPath: try a.required("--stop")), intervalMs: ms)
            return .ok
        case "ax-judge":
            let v = judgeAxWindows(try readAxSamples(try a.required("--samples")))
            try writeJSON(v, to: nil)
            return v.ok ? .ok : ((v.byState[AxWindowsState.detached.rawValue] ?? 0) > 0 ? .violation : .unmeasured)
        default:
            throw GuardError.usage("知らない副命令 \(sub)")
        }
    }

    /// events.jsonl を読む。空の行は飛ばす。壊れた行は badInput（黙って捨てない）。無い file は空（judge が watcher_not_running にする）。
    static func readEvents(_ path: String) throws -> [Event] {
        guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return [] }
        return try text.split(separator: "\n").enumerated().map { i, line in
            do { return try RecordJSON.decoder().decode(Event.self, from: Data(line.utf8)) }
            catch { throw GuardError.badInput(path: path, reason: "line \(i + 1): \(error)") }
        }
    }
}

// MARK: - snap

/// 今の 1 枚を取る（前面の app・cursor・system-wide の focus・on screen の窓の z 順）。
/// 使い方: `front-guard snap --out <path> [--needle <text>]` → Snapshot の JSON を書いて exit 0。
/// 保証する: 前面も cursor も focus も変えない（読むだけ・activate しない）。許可が無い検出は `unmeasured` に理由を書く。
/// 保証しない: needle の照合は AXValue が String の element だけ（secure field・WebView の中は needle_present = nil → leak は no_needle の未測）。
func takeSnapshot(needle: String?, pids: [Int32] = []) throws -> Snapshot {
    var un: [Detection: Unmeasured] = [:]
    let front = NSWorkspace.shared.frontmostApplication.map {
        AppRef(pid: $0.processIdentifier, bundleId: $0.bundleIdentifier, name: $0.localizedName)
    }
    let loc = CGEvent(source: nil)?.location ?? .zero
    var focus: FocusRef?
    if AXIsProcessTrusted() {
        focus = readFocus(needle: needle)
        if needle != nil, focus?.needlePresent == nil { un[.leak] = .noNeedle }
    } else {
        un[.focus] = .accessibilityDenied
        if needle != nil { un[.leak] = .accessibilityDenied }
    }
    let windows = readWindows()
    let targets: [TargetState]? = pids.isEmpty ? nil : pids.compactMap { p in
        NSRunningApplication(processIdentifier: p).map { a in
            TargetState(pid: p, name: a.localizedName, hidden: a.isHidden, onscreenWindows: windows.filter { $0.pid == p && $0.layer == 0 }.count)
        }
    }
    return Snapshot(schema: .snapshot, t: Date().timeIntervalSince1970, front: front, cursor: Point(x: loc.x, y: loc.y),
                    focus: focus, windows: windows, unmeasured: un, targets: targets)
}

/// system-wide の AXFocusedUIElement を読む。focus が無い・読めない時は nil（前後とも nil なら同じと見る）。
func readFocus(needle: String?) -> FocusRef? {
    let sys = AXUIElementCreateSystemWide()
    AXUIElementSetMessagingTimeout(sys, 1.0)
    var ref: CFTypeRef?
    guard AXUIElementCopyAttributeValue(sys, kAXFocusedUIElementAttribute as CFString, &ref) == .success, let el = ref else { return nil }
    let e = el as! AXUIElement
    var pid: pid_t = 0
    AXUIElementGetPid(e, &pid)
    func attr(_ n: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(e, n as CFString, &v) == .success ? v : nil
    }
    var frame: Rect?
    if let p = attr(kAXPositionAttribute), let s = attr(kAXSizeAttribute) {
        var pt = CGPoint.zero, sz = CGSize.zero
        if AXValueGetValue(p as! AXValue, .cgPoint, &pt), AXValueGetValue(s as! AXValue, .cgSize, &sz) {
            frame = Rect(x: pt.x, y: pt.y, w: sz.width, h: sz.height)
        }
    }
    // 値そのものは残さない（owner の文字を記録しない）。needle を含むかの Bool だけ。
    let present = needle.flatMap { n in (attr(kAXValueAttribute) as? String).map { $0.contains(n) } }
    return FocusRef(pid: pid, role: attr(kAXRoleAttribute) as? String, subrole: attr(kAXSubroleAttribute) as? String,
                    identifier: attr(kAXIdentifierAttribute) as? String, frame: frame, needlePresent: present)
}

/// CGWindowListCopyWindowInfo(.optionOnScreenOnly) を z 順（前から）で読む。oracle/windows と同じ読み方（lib の readWindowList）。
/// 保証する: layer 0 以外（menu bar・Dock）も含めて返す。judge が layer で絞る。
func readWindows() -> [WindowInfo] { readWindowList() }

// MARK: - watch

/// run の間ずっと記録する。2（activated）と 4（space_changed）は通知で、3 の標本は interval ごとに events.jsonl へ 1 行ずつ追記。
/// 使い方: `front-guard watch --out <events.jsonl> --stop <stopfile> [--suspect <pid>…] [--interval-ms 200]`。stopfile が出来たら kind=stop を書いて exit 0。
/// 保証する: 最初の行は kind=start・最後の行は kind=stop（stop が無い events を judge は watcher_not_running の未測にする）。
///   z_sample は変化が無くても毎 interval 書く（watcher が手の終わりまで生きていた証拠を judge が読む）。窓は suspect の窓と、それと交わる layer 0 の窓だけ（全窓を毎回書くと run が数十 MB になる）。
/// 保証しない: suspect の pid は記録の印に使うだけで、kill しない（kill は run.sh の判断）。
func watch(out: URL, stop: URL, suspects: [Int32], intervalMs: Int) throws {
    if !FileManager.default.fileExists(atPath: out.path) {
        guard FileManager.default.createFile(atPath: out.path, contents: nil) else { throw GuardError.badInput(path: out.path, reason: "作れない") }
    }
    let fh = try FileHandle(forWritingTo: out)
    fh.seekToEndOfFile()
    let enc = RecordJSON.encoder()
    func emit(_ kind: Event.Kind, app: AppRef? = nil, windows: [WindowInfo]? = nil) {
        guard var d = try? enc.encode(Event(schema: .event, t: Date().timeIntervalSince1970, kind: kind, app: app, windows: windows)) else { return }
        d.append(0x0A)
        fh.write(d)
    }
    emit(.start)
    let nc = NSWorkspace.shared.notificationCenter
    nc.addObserver(forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { n in
        let a = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
        emit(.activated, app: a.map { AppRef(pid: $0.processIdentifier, bundleId: $0.bundleIdentifier, name: $0.localizedName) })
    }
    nc.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { _ in emit(.spaceChanged) }
    let sus = Set(suspects)
    Timer.scheduledTimer(withTimeInterval: Double(intervalMs) / 1000, repeats: true) { _ in
        if FileManager.default.fileExists(atPath: stop.path) {
            emit(.stop)
            try? fh.close()
            exit(ExitCode.ok.rawValue)
        }
        let all = readWindowList()
        let mine = all.filter { sus.contains($0.pid) }
        emit(.zSample, windows: all.filter { w in sus.contains(w.pid) || (w.layer == 0 && mine.contains { $0.bounds.intersects(w.bounds) }) })
    }
    RunLoop.main.run()
}

// MARK: - judge

/// 1 手の判定。純関数（画面・ファイルに触らない）。events は [before.t, after.t] の間の行だけを見る。
/// 使い方: `front-guard judge --step <name> --before <a.json> --after <b.json> --events <events.jsonl> --suspect <pid>… [--owner <pid>]`
///   --owner は run の始め（起動の前）に記録した owner の前面の pid。無ければ before.front を owner と見る
///   （手の間に前面を取られると before.front が Shibubu になり何も赤くならないので、run.sh は必ず渡す）。
///   → Verdict を stdout に書く。exit は verdictExit(_:)。
/// 規則（D-g の番号）:
///   1 front: before.front.pid か after.front.pid が owner でない、または cursor が 0.5pt 以上動いた
///   2 activation: 間に kind=activated が 1 件でもあり、その app.pid が before.front.pid でない
///   3 zorder: before / after / 間の z_sample のどれかで、suspect の窓が owner（before.front）の layer 0 の窓と交わり、z が小さい（上にある）
///   4 space: 間に kind=space_changed が 1 件でも
///   5 focus: before.focus != after.focus（pid・role・subrole・identifier・frame の組）。前後とも nil は no_focus_element の未測
///   owner_frame: owner の layer 0 の窓の bounds が window_id ごとに before と after で同じでない（窓が消えた時は数えない）
///   leak: after.focus.needle_present == true（before で既に true なら除く）
/// watcher の生存: start が before.t 以前・after.t 以降の行が在る・間に stop が無い。満たさなければ 2・3・4 は watcher_not_running の未測。
/// leak は snap に --needle を渡した手だけの検出（渡さない手は measured にも unmeasured にも入れない）。
/// 保証する: 測れない検出は violation 0 でも unmeasured に入り、ok は false。
public func judge(step: String, before: Snapshot, after: Snapshot, events: [Event], suspects: [Int32], owner ownerArg: Int32? = nil) -> Verdict {
    var vs: [Violation] = []
    var un = before.unmeasured.merging(after.unmeasured) { a, _ in a }
    func add(_ d: Detection, _ t: Double, _ s: String) { vs.append(Violation(detection: d, t: t, detail: s)) }
    func who(_ a: AppRef?) -> String { a.map { "\($0.pid)(\($0.name ?? "?"))" } ?? "none" }
    let lo = before.t, hi = after.t
    let inside = events.filter { $0.t >= lo && $0.t <= hi }
    let watcherOK = events.contains { $0.kind == .start && $0.t <= lo } && events.contains { $0.t >= hi }
        && !events.contains { $0.kind == .stop && $0.t < hi }
    if !watcherOK { for d in [Detection.activation, .zorder, .space] where un[d] == nil { un[d] = .watcherNotRunning } }
    let owner = ownerArg ?? before.front?.pid

    // 1 front
    if let o = owner, let b = before.front?.pid, let a = after.front?.pid {
        if b != o || a != o { add(.front, hi, "front \(who(before.front)) -> \(who(after.front)) (owner \(o))") }
    } else if un[.front] == nil { un[.front] = .noOwnerFront }
    let dx = after.cursor.x - before.cursor.x, dy = after.cursor.y - before.cursor.y
    if (dx * dx + dy * dy).squareRoot() >= 0.5 {
        add(.front, hi, "cursor (\(before.cursor.x),\(before.cursor.y)) -> (\(after.cursor.x),\(after.cursor.y))（owner の手でも赤）")
    }

    // 2 activation（一瞬の前面化も 1 件）
    for e in inside where e.kind == .activated && e.app?.pid != owner {
        add(.activation, e.t, "activated \(who(e.app)) (owner \(owner.map(String.init) ?? "none"))")
    }

    // 3 zorder
    if let o = owner {
        let sus = Set(suspects)
        let sets = [(lo, before.windows), (hi, after.windows)] + inside.compactMap { e in e.kind == .zSample ? e.windows.map { (e.t, $0) } : nil }
        for (t, ws) in sets {
            let ow = ws.filter { $0.pid == o && $0.layer == 0 && $0.z >= 0 }
            for s in ws where sus.contains(s.pid) && s.z >= 0 {
                if let w = ow.first(where: { s.z < $0.z && s.bounds.intersects($0.bounds) }) {
                    add(.zorder, t, "window \(s.windowId)(pid \(s.pid)) z\(s.z) over owner window \(w.windowId) z\(w.z)")
                }
            }
        }
    } else if un[.zorder] == nil { un[.zorder] = .noOwnerFront }

    // 4 space
    for e in inside where e.kind == .spaceChanged { add(.space, e.t, "active space changed") }

    // 5 focus（needle_present は比べない）
    if un[.focus] == nil, before.focus == nil, after.focus == nil { un[.focus] = .noFocusElement }
    if un[.focus] == nil {
        func key(_ f: FocusRef?) -> FocusRef? { f.map { var g = $0; g.needlePresent = nil; return g } }
        if key(before.focus) != key(after.focus) {
            add(.focus, hi, "focus \(before.focus.map { "\($0.pid) \($0.role ?? "?")" } ?? "none") -> \(after.focus.map { "\($0.pid) \($0.role ?? "?")" } ?? "none")")
        }
    }

    // owner_frame
    if let o = owner {
        let after_ = Dictionary(after.windows.map { ($0.windowId, $0.bounds) }, uniquingKeysWith: { a, _ in a })
        for w in before.windows where w.pid == o && w.layer == 0 {
            if let b = after_[w.windowId], abs(b.x - w.bounds.x) + abs(b.y - w.bounds.y) + abs(b.w - w.bounds.w) + abs(b.h - w.bounds.h) >= 0.5 {
                add(.ownerFrame, hi, "owner window \(w.windowId) \(w.bounds) -> \(b)")
            }
        }
    } else if un[.ownerFrame] == nil { un[.ownerFrame] = .noOwnerFront }

    // leak
    let leakApplies = un[.leak] != nil || after.focus?.needlePresent != nil
    if after.focus?.needlePresent == true, before.focus?.needlePresent != true {
        add(.leak, hi, "needle appeared in focused element of pid \(after.focus!.pid) (\(after.focus!.role ?? "?"))")
    }

    let scope = Detection.allCases.filter { $0 != .leak || leakApplies }
    return Verdict(schema: .verdict, step: step, ok: vs.isEmpty && un.isEmpty, violations: vs,
                   measured: scope.filter { un[$0] == nil }, unmeasured: un)
}

/// Verdict → exit code。violation があれば 1、無くて未測があれば 6、どちらも無ければ 0。
public func verdictExit(_ v: Verdict) -> ExitCode {
    !v.violations.isEmpty ? .violation : (!v.unmeasured.isEmpty ? .unmeasured : .ok)
}

// MARK: - provoke（負の対照・FG 枠の中でだけ）

/// わざと 1 つの検出を赤にする小さな動き（この process 自身が前面を取る・窓を重ねる・cursor を動かす・focus を取る）。
/// 使い方: `front-guard provoke <detection> --fg-window`（§8 ⓪・owner が PC を離れている間だけ）。--fg-window が無ければ
///   GuardError.fgWindowRequired で何もせず exit 5。`space` は Mission Control の手が要るので provoke しない（FG で owner の手）。
/// 保証する: 動いた後に元へ戻す試みはしない（戻す操作もまた前面を変えるため・YC_WAVES W2b-2「代わりの道」）。
/// 保証しない: leak の provoke は無い（作った snapshot で judge を赤にする BG の対照で足りる）。
func provoke(_ detection: Detection, fgWindow: Bool) throws {
    guard fgWindow else { throw GuardError.fgWindowRequired }
    func pump(_ s: Double) { RunLoop.current.run(until: Date().addingTimeInterval(s)) }
    switch detection {
    case .front:
        let p = CGEvent(source: nil)?.location ?? .zero
        CGWarpMouseCursorPosition(CGPoint(x: p.x + 40, y: p.y + 40))
    case .ownerFrame:
        guard let f = NSWorkspace.shared.frontmostApplication else { throw GuardError.permission(.noOwnerFront) }
        guard AXIsProcessTrusted() else { throw GuardError.permission(.accessibilityDenied) }
        var w: CFTypeRef?, pos: CFTypeRef?
        let app = AXUIElementCreateApplication(f.processIdentifier)
        guard AXUIElementCopyAttributeValue(app, kAXFocusedWindowAttribute as CFString, &w) == .success,
              AXUIElementCopyAttributeValue(w as! AXUIElement, kAXPositionAttribute as CFString, &pos) == .success else {
            throw GuardError.permission(.noOwnerFront)
        }
        var pt = CGPoint.zero
        AXValueGetValue(pos as! AXValue, .cgPoint, &pt)
        pt.x += 1
        AXUIElementSetAttributeValue(w as! AXUIElement, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &pt)!)
    case .activation, .zorder, .focus:
        // 重ねる先は owner の前面の窓（layer 0 の一番前）
        let owner = NSWorkspace.shared.frontmostApplication?.processIdentifier
        let target = readWindowList().first { $0.pid == owner && $0.layer == 0 }?.bounds ?? Rect(x: 100, y: 100, w: 300, h: 200)
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let screenH = NSScreen.screens.first?.frame.height ?? 0
        let win = NSWindow(contentRect: NSRect(x: target.x, y: screenH - target.y - min(target.h, 200), width: min(target.w, 300), height: min(target.h, 200)),
                           styleMask: [.titled], backing: .buffered, defer: false)
        win.title = "front-guard provoke \(detection.rawValue)"
        if detection == .zorder {
            win.orderFrontRegardless()  // 前面は取らず窓だけ上に
        } else {
            let field = NSTextField(frame: NSRect(x: 10, y: 10, width: 200, height: 24))
            win.contentView?.addSubview(field)
            app.activate(ignoringOtherApps: true)
            win.makeKeyAndOrderFront(nil)
            win.makeFirstResponder(field)
        }
        pump(1.0)
    case .space, .leak:
        throw GuardError.usage("\(detection.rawValue) は provoke しない（space は FG の owner の手・leak は作った snapshot の judge）")
    }
}

// MARK: - W2b-AX: AX の窓の数（外から）

/// `AXUIElementCreateApplication(pid)` の `kAXWindows` を timeoutS（`AXUIElementSetMessagingTimeout`）で 1 回読み、同じ時刻の
/// `readWindowList(pid:)` の layer 0・onscreen の数と並べて `classifyAxWindows` で state を付ける。
/// 保証する: 読むだけ（AX の action を送らない・前面を変えない）。許可が無ければ GuardError.permission(.accessibilityDenied)。
/// 保証しない: Shibubu の木の中身（窓の数だけ）。
func readAxWindows(pid: Int32, variant: String?, timeoutS: Double) throws -> AxWindowsReading {
    guard AXIsProcessTrusted() else { throw GuardError.permission(.accessibilityDenied) }
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, Float(timeoutS))
    let t0 = Date()
    var ref: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &ref)
    let secs = Date().timeIntervalSince(t0)
    let count = err == .success ? (ref as? [AXUIElement])?.count ?? 0 : nil
    let cg = readWindowList(pid: pid).filter { $0.layer == 0 && $0.onscreen }.count
    var la = [Double](repeating: 0, count: 3)
    let load1 = getloadavg(&la, 3) > 0 ? la[0] : nil
    return AxWindowsReading(schema: .axWindows, t: t0.timeIntervalSince1970, pid: pid, variant: variant, count: count, err: err.rawValue,
                            secs: secs, cgOnscreen: cg, load1: load1, state: classifyAxWindows(count: count, err: err.rawValue, cgOnscreen: cg))
}

/// stopfile が出来るまで interval ごとに readAxWindows を 1 行 1 件で追記する（1 回の読みが interval より長い時は詰めて続ける）。
func sampleAxWindows(pid: Int32, variant: String?, out: URL, stop: URL, intervalMs: Int) throws {
    if !FileManager.default.fileExists(atPath: out.path) {
        guard FileManager.default.createFile(atPath: out.path, contents: nil) else { throw GuardError.badInput(path: out.path, reason: "作れない") }
    }
    let fh = try FileHandle(forWritingTo: out)
    defer { try? fh.close() }
    fh.seekToEndOfFile()
    let enc = RecordJSON.encoder()
    while !FileManager.default.fileExists(atPath: stop.path) {
        let t0 = Date()
        var d = try enc.encode(try readAxWindows(pid: pid, variant: variant, timeoutS: 10))
        d.append(0x0A)
        fh.write(d)
        let rest = Double(intervalMs) / 1000 - Date().timeIntervalSince(t0)
        if rest > 0 { Thread.sleep(forTimeInterval: rest) }
    }
}

/// jsonl を読む。空の行は飛ばす・壊れた行と schema の違う行は badInput（黙って捨てない）。
func readAxSamples(_ path: String) throws -> [AxWindowsReading] {
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { throw GuardError.badInput(path: path, reason: "読めない") }
    return try text.split(separator: "\n").enumerated().compactMap { i, line in
        if line.allSatisfy(\.isWhitespace) { return nil }
        do {
            let r = try RecordJSON.decoder().decode(AxWindowsReading.self, from: Data(line.utf8))
            guard r.schema == .axWindows else { throw GuardError.badInput(path: path, reason: "line \(i + 1): schema \(r.schema.rawValue)") }
            return r
        } catch let e as GuardError { throw e } catch { throw GuardError.badInput(path: path, reason: "line \(i + 1): \(error)") }
    }
}

/// 純関数。ok = detached 0 件 かつ unresponsive 0 件 かつ attached 1 件以上（0 件・error / no_window だけの緑にしない）。
/// noWindow は窓が出る前なら ok を落とさない。最初の attached の後の noWindow は run の途中で窓（process）が消えた＝未測（ok=false・exit 6）。
/// run.sh の stop は sampler を止めてから kill するので、普通の終わりは noWindow を残さない。
/// 行に書いてある `state` は使わず、count・err・cg_onscreen から `classifyAxWindows` で決め直す（判定の分岐を書いた値の鵜呑みにしない）。
func judgeAxWindows(_ samples: [AxWindowsReading]) -> AxWindowsVerdict {
    let states = samples.map { classifyAxWindows(count: $0.count, err: $0.err, cgOnscreen: $0.cgOnscreen) }
    var by: [String: Int] = [:]
    for st in states { by[st.rawValue, default: 0] += 1 }
    let vanished = states.drop { $0 != .attached }.contains(.noWindow)
    let ok = !vanished && by[AxWindowsState.attached.rawValue] != nil && by[AxWindowsState.detached.rawValue] == nil && by[AxWindowsState.unresponsive.rawValue] == nil
    return AxWindowsVerdict(schema: .axWindowsVerdict, samples: samples.count, byState: by,
                            firstDetached: zip(samples, states).first { $0.1 == .detached }?.0.t, ok: ok)
}

// MARK: - ax（W12a: 試験の側の AX の木・読むだけ）

/// AX の木の 1 要素（cua-driver の get_window_state の `elements` と同じ鍵・足したのは `identifier`）。
struct AXElementRec: Encodable {
    let element_index: Int
    let parent_index: Int?
    let depth: Int
    let role: String?
    let label: String?
    let value: String?
    let identifier: String?
}

struct AXTreeRec: Encodable {
    let schema = "shibubu.qa.axtree/1"
    let t: Double
    let pid: Int32
    let elements: [AXElementRec]
    let truncated: Bool
}

/// pid の app の窓から下の AX の木を、親が子より前の順（深さ優先）で読む。cua-driver の get_window_state は AXIdentifier を返さないので、
/// identifier（accesskit の author_id・`data:<node key>` 等）で部分木を引く手（tally.py の git_seen）はこれで読む。
/// 保証する: 読むだけ（AXUIElementCopyAttributeValue だけ・Set も Perform も呼ばない）・前面も focus も変えない・`limit` 個で打ち切り truncated。
/// 保証しない: 読む間の木の変化（1 回の walk の間に描き直されると混ざる）。Accessibility の許可が無ければ GuardError.permission（exit 3）。
func axTree(pid: pid_t, limit: Int) throws -> AXTreeRec {
    guard AXIsProcessTrusted() else { throw GuardError.permission(.accessibilityDenied) }
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 2.0)
    func attr(_ e: AXUIElement, _ n: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(e, n as CFString, &v) == .success ? v : nil
    }
    func str(_ e: AXUIElement, _ n: String) -> String? {
        (attr(e, n) as? String).flatMap { $0.isEmpty ? nil : $0 }
    }
    var out: [AXElementRec] = []
    var truncated = false
    var stack: [(AXUIElement, Int?, Int)] = ((attr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? []).reversed().map { ($0, nil, 0) }
    while let (e, parent, depth) = stack.popLast() {
        if out.count >= limit { truncated = true; break }
        let i = out.count
        let value: String? = attr(e, kAXValueAttribute).flatMap { v in (v as? String) ?? (v as? NSNumber)?.stringValue }
        out.append(AXElementRec(element_index: i, parent_index: parent, depth: depth, role: str(e, kAXRoleAttribute),
                                label: str(e, kAXTitleAttribute) ?? str(e, kAXDescriptionAttribute), value: value,
                                identifier: str(e, "AXIdentifier")))
        for k in ((attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? []).reversed() { stack.append((k, i, depth + 1)) }
    }
    return AXTreeRec(t: Date().timeIntervalSince1970, pid: pid, elements: out, truncated: truncated)
}
