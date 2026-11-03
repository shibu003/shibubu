// oracle/ax — 試験の側の AX / 窓 / OCR の道具を 1 本に寄せた物（読む・置く・押す。製品の src には入れない）。
//   ax id <pid> <identifier の頭> [timeout 秒]   窓の AX の木を幅優先で辿り、AXIdentifier が頭で始まる要素を 1 行 1 JSON（identifier・role・value・label・frame）。exit 3 = Accessibility の許可が無い。stderr の最後の 1 行 `ax: windows=<N> visited=<N>`（窓が 0〈app が答えない〉と該当なしを分ける）
//   ax tree <pid> <窓 id>                        action の無い Group も含めた木を `{"window":{x,y,w,h},"elements":[{identifier,role,frame}]}` で（cua-driver の木は Group を落とす・W16M-2）
//   ax frame <pid> <窓の題名> <x> <y> <w> <h>     窓（AXTitle が同じ）を指定の位置と大きさにして main にする（activate しない）
//   ax sel <pid> <窓の題名>                       窓（AXTitle が頭で始まる）の最初の AXTextArea の選択の範囲を `<loc> <len>` で（読むだけ）
//   ax set <pid> <窓の題名> <文字>                同じ text area の AXValue を替える（activate しない・混んだ機械は 10 回まで待つ）
//   ax press <pid> <窓 id> <AXIdentifier>        hit test と親を辿って AXIdentifier が一致する要素だけ AXPress（`{"pressed":true,…}`）
//   ax windows                                   画面の上の窓（layer 0）を前から `<順位>\t<pid>\t<窓 id>\t<x>,<y>,<w>,<h>\t<app 名>` で（guard.sh）
//   ax ocr <png> <window_id> <x> <y> <w> <h>     写真の矩形（窓の point）を Vision で読み、行の JSON 配列を 1 行（読めない時は空の配列で成功にしない）
// 使い方 / 窓・要素が無い / AX が断った = exit 2 と stderr に理由（id の許可無しだけ 3）。build: `xcrun swiftc -O oracle/ax.swift -o <出力>`（build.sh には入れない＝front-guard の許可に触れない）。
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import Vision

func fail(_ m: String, _ code: Int32 = 2) -> Never { FileHandle.standardError.write((m + "\n").data(using: .utf8)!); exit(code) }
func attr(_ e: AXUIElement, _ k: String) -> AnyObject? { var v: CFTypeRef?; return AXUIElementCopyAttributeValue(e, k as CFString, &v) == .success ? v : nil }
func str(_ v: AnyObject?) -> String? {
    if let s = v as? String { return s }
    if let a = v as? NSAttributedString { return a.string }
    return nil
}
func point(_ e: AXUIElement) -> CGPoint? {
    guard let v = attr(e, kAXPositionAttribute) else { return nil }
    var p = CGPoint.zero
    return AXValueGetValue(v as! AXValue, .cgPoint, &p) ? p : nil
}
func size(_ e: AXUIElement) -> CGSize? {
    guard let v = attr(e, kAXSizeAttribute) else { return nil }
    var s = CGSize.zero
    return AXValueGetValue(v as! AXValue, .cgSize, &s) ? s : nil
}
func appWindows(_ app: AXUIElement) -> [AXUIElement] { (attr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? [] }
func title(_ e: AXUIElement) -> String { (attr(e, kAXTitleAttribute) as? String) ?? "" }
func textArea(_ e: AXUIElement, _ d: Int = 0) -> AXUIElement? {
    if (attr(e, kAXRoleAttribute) as? String) == kAXTextAreaRole { return e }
    if d > 12 { return nil }
    for c in (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] { if let f = textArea(c, d + 1) { return f } }
    return nil
}
func windowRect(_ wid: UInt32) -> CGRect {
    guard let info = (CGWindowListCopyWindowInfo([.optionIncludingWindow], CGWindowID(wid)) as? [[String: Any]])?.first(where: { ($0[kCGWindowNumber as String] as? Int) == Int(wid) }),
          let b = info[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: b as CFDictionary)
    else { fail("窓 \(wid) がありません") }
    return r
}
func json(_ o: Any) -> String { String(data: try! JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]), encoding: .utf8)! }

func cmdId(_ a: [String]) {
    guard a.count >= 2, let pid = pid_t(a[0]) else { fail("使い方: ax id <pid> <identifier の頭> [timeout 秒]") }
    guard AXIsProcessTrusted() else { fail("ax: アクセシビリティの許可がありません", 3) }
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, Float(a.count > 2 ? Double(a[2]) ?? 20 : 20))
    var queue = appWindows(app)
    let windows = queue.count
    var visited = 0
    while !queue.isEmpty && visited < 5000 {
        let e = queue.removeFirst()
        visited += 1
        if let id = str(attr(e, "AXIdentifier")), id.hasPrefix(a[1]) {
            var o: [String: Any] = ["identifier": id]
            if let r = str(attr(e, kAXRoleAttribute)) { o["role"] = r }
            if let v = str(attr(e, kAXValueAttribute)) { o["value"] = v }
            if let l = str(attr(e, kAXDescriptionAttribute)) ?? str(attr(e, kAXTitleAttribute)) { o["label"] = l }
            if let p = point(e), let s = size(e) { o["frame"] = ["x": p.x, "y": p.y, "w": s.width, "h": s.height] }
            print(json(o))
        }
        queue += (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? []
    }
    FileHandle.standardError.write("ax: windows=\(windows) visited=\(visited)\n".data(using: .utf8)!)
}

func cmdTree(_ a: [String]) {
    guard a.count == 2, let pid = pid_t(a[0]), let wid = UInt32(a[1]) else { fail("使い方: ax tree <pid> <窓 id>") }
    let wrect = windowRect(wid)
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 10)
    let windows = appWindows(app)
    guard !windows.isEmpty else { fail("pid \(pid) の AX の窓がありません") }
    let axwin: AXUIElement = {
        if let hit = windows.first(where: { (attr($0, "AXWindowNumber") as? NSNumber)?.uint32Value == wid }) { return hit }
        // 番号が取れない時は位置と大きさで突き合わせる（1 つに決まる時だけ）
        let matches = windows.filter { e in
            guard let p = point(e), let s = size(e) else { return false }
            return abs(p.x - wrect.minX) < 2 && abs(p.y - wrect.minY) < 2 && abs(s.width - wrect.width) < 2 && abs(s.height - wrect.height) < 2
        }
        guard matches.count == 1 else { fail("窓 \(wid) の AX の窓が決まりません（\(matches.count)）") }
        return matches[0]
    }()
    var elems: [(id: String, role: String, p: CGPoint, s: CGSize)] = []
    func walk(_ e: AXUIElement, _ depth: Int) {
        if depth > 40 { return }
        let role = (attr(e, kAXRoleAttribute) as? String) ?? ""
        let id = (attr(e, kAXIdentifierAttribute) as? String) ?? ""
        if let p = point(e), let s = size(e), (!id.isEmpty || role == "AXGroup") { elems.append((id, role, p, s)) }
        for c in (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] { walk(c, depth + 1) }
    }
    walk(axwin, 0)
    func num(_ d: Double) -> String { d == d.rounded() ? String(Int(d.rounded())) : String(format: "%.1f", d) }
    func esc(_ s: String) -> String { s.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") }
    var out = #"{"window":{"x":\#(num(wrect.minX)),"y":\#(num(wrect.minY)),"w":\#(num(wrect.width)),"h":\#(num(wrect.height))},"elements":["#
    for (i, e) in elems.enumerated() {
        if i > 0 { out += "," }
        out += #"{"identifier":"\#(esc(e.id))","role":"\#(esc(e.role))","frame":{"x":\#(num(e.p.x)),"y":\#(num(e.p.y)),"w":\#(num(e.s.width)),"h":\#(num(e.s.height))}}"#
    }
    print(out + "]}")
}

func cmdFrame(_ a: [String]) {
    guard a.count == 6, let pid = pid_t(a[0]), let x = Double(a[2]), let y = Double(a[3]), let w = Double(a[4]), let h = Double(a[5])
    else { fail("使い方: ax frame <pid> <窓の題名> <x> <y> <w> <h>") }
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 5)
    guard let win = appWindows(app).first(where: { title($0) == a[1] }) else { fail("窓「\(a[1])」がありません") }
    var s = CGSize(width: w, height: h), p = CGPoint(x: x, y: y)
    for (k, v) in [(kAXSizeAttribute, AXValueCreate(.cgSize, &s)!), (kAXPositionAttribute, AXValueCreate(.cgPoint, &p)!)] {
        let r = AXUIElementSetAttributeValue(win, k as CFString, v)
        guard r == .success else { fail("\(k) を置けません（\(r.rawValue)）") }
    }
    let r = AXUIElementSetAttributeValue(win, kAXMainAttribute as CFString, kCFBooleanTrue)
    guard r == .success else { fail("main にできません（\(r.rawValue)）") }
}

func cmdSel(_ a: [String]) {
    guard a.count == 2, let pid = pid_t(a[0]) else { fail("使い方: ax sel <pid> <窓の題名>") }
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 5)
    guard let win = appWindows(app).first(where: { title($0).hasPrefix(a[1]) }) else { fail("窓「\(a[1])」がありません") }
    guard let t = textArea(win), let v = attr(t, kAXSelectedTextRangeAttribute), CFGetTypeID(v) == AXValueGetTypeID() else { fail("選択の範囲を読めません") }
    var r = CFRange()
    guard AXValueGetValue(v as! AXValue, .cfRange, &r) else { fail("選択の範囲を読めません") }
    print("\(r.location) \(r.length)")
}

func cmdSet(_ a: [String]) {
    guard a.count == 3, let pid = pid_t(a[0]) else { fail("使い方: ax set <pid> <窓の題名> <文字>") }
    let app = AXUIElementCreateApplication(pid)
    // 混んだ機械では AX の答えが既定の待ちを越えて空になる。待ちを延ばし、窓が見えるまで 10 回まで
    AXUIElementSetMessagingTimeout(app, 5)
    var found: AXUIElement?
    for _ in 0..<10 {
        found = appWindows(app).first(where: { title($0).hasPrefix(a[1]) })
        if found != nil { break }
        usleep(500_000)
    }
    guard let win = found else { fail("窓「\(a[1])」がありません") }
    guard let t = textArea(win) else { fail("text area がありません") }
    var r = AXError.cannotComplete
    for _ in 0..<10 {
        r = AXUIElementSetAttributeValue(t, kAXValueAttribute as CFString, a[2] as CFString)
        if r != .cannotComplete { break }
        usleep(500_000)
    }
    guard r == .success else { fail("AXValue を書けません（\(r.rawValue)）") }
}

func cmdPress(_ a: [String]) {
    guard a.count == 3, let pid = pid_t(a[0]), let wid = UInt32(a[1]) else { fail("使い方: ax press <pid> <窓 id> <AXIdentifier>") }
    let rect = windowRect(wid)
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 10)
    var tries = 0
    for fy in [0.5, 0.3, 0.7, 0.15, 0.85] {
        for fx in [0.5, 0.3, 0.7, 0.15, 0.85] {
            tries += 1
            var hit: AXUIElement?
            guard AXUIElementCopyElementAtPosition(app, Float(rect.minX + fx * rect.width), Float(rect.minY + fy * rect.height), &hit) == .success, var e = hit else { continue }
            for _ in 0..<8 {
                if (attr(e, "AXIdentifier") as? String) == a[2] {
                    let r = AXUIElementPerformAction(e, kAXPressAction as CFString)
                    guard r == .success else { fail("AXPress が \(r.rawValue)") }
                    print(#"{"pressed":true,"role":"\#((attr(e, kAXRoleAttribute) as? String) ?? "")","tries":\#(tries)}"#)
                    exit(0)
                }
                guard let p = attr(e, kAXParentAttribute), CFGetTypeID(p) == AXUIElementGetTypeID() else { break }
                e = p as! AXUIElement
            }
        }
    }
    fail("AXIdentifier \(a[2]) の要素が窓 \(wid) の中に見つかりません（\(tries) 点）")
}

func cmdWindows() {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    var i = 0
    for w in list where (w[kCGWindowLayer as String] as? Int) == 0 {
        let b = w[kCGWindowBounds as String] as? [String: Double] ?? [:]
        print("\(i)\t\(w[kCGWindowOwnerPID as String] as? Int ?? -1)\t\(w[kCGWindowNumber as String] as? Int ?? -1)\t\(Int(b["X"] ?? 0)),\(Int(b["Y"] ?? 0)),\(Int(b["Width"] ?? 0)),\(Int(b["Height"] ?? 0))\t\(w[kCGWindowOwnerName as String] as? String ?? "?")")
        i += 1
    }
}

func cmdOcr(_ a: [String]) {
    guard a.count == 6, let wid = UInt32(a[1]), let x = Double(a[2]), let y = Double(a[3]), let w = Double(a[4]), let h = Double(a[5]) else {
        fail("使い方: ax ocr <png> <window_id> <x> <y> <w> <h>")
    }
    guard let img = NSImage(contentsOfFile: a[0])?.cgImage(forProposedRect: nil, context: nil, hints: nil) else { fail("写真を読めません: \(a[0])") }
    let pw = windowRect(wid).width
    guard pw > 0 else { fail("窓 \(wid) がありません") }
    let s = Double(img.width) / pw
    let r = CGRect(x: x * s, y: y * s, width: w * s, height: h * s).integral
    guard r.width > 0, r.height > 0, let crop = img.cropping(to: r) else { fail("矩形 \(r) が写真（\(img.width)×\(img.height)）の外です") }
    let req = VNRecognizeTextRequest()
    req.recognitionLevel = .accurate
    req.usesLanguageCorrection = false
    do { try VNImageRequestHandler(cgImage: crop).perform([req]) } catch { fail("Vision: \(error)") }
    print(json((req.results ?? []).compactMap { $0.topCandidates(1).first?.string }))
}

let argv = Array(CommandLine.arguments.dropFirst())
let rest = Array(argv.dropFirst())
switch argv.first {
case "id": cmdId(rest)
case "tree": cmdTree(rest)
case "frame": cmdFrame(rest)
case "sel": cmdSel(rest)
case "set": cmdSet(rest)
case "press": cmdPress(rest)
case "windows": cmdWindows()
case "ocr": cmdOcr(rest)
default: fail("使い方: ax <id|tree|frame|sel|set|press|windows|ocr> …")
}
