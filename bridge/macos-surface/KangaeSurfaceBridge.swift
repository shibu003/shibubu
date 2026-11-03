// Kangae の macOS surface bridge（v0.14 §5.5）。Rust（src/surface）が子 process として起こし、
// stdin に 1 行 1 つの JSON の要求 `{"id":N,"cmd":"…",…}`、stdout に 1 行 1 つの JSON を返す:
//   応答 `{"id":N,"ok":true,"result":{…}}` / `{"id":N,"ok":false,"error":{"code":"…","message":"…"}}`
//   event `{"event":"moved",…}`（id を持たない。AXObserver と NSWorkspace の通知）
// 公開 API だけを使う（NSWorkspace・CGWindowListCopyWindowInfo・AXUIElement / AXObserver・AXIsProcessTrustedWithOptions・CGEvent.postToPid）。
// 非公開 API（SkyLight / CGS・_AXUIElementGetWindow）と他の process への code 注入は使わない。そのため AX の窓と CG の窓番号の
// 対応は「同じ pid で frame が一致する窓が 1 つだけ」の時だけ付ける（cg_match = frame / none / ambiguous）。
// できない事は明示の error で返す（黙って成功にしない）。stdin が閉じたら終わる（親が死んだ時に残らない）。
// build: swiftc -O -swift-version 5 bridge/macos-surface/KangaeSurfaceBridge.swift -o target/bridge/kangae-surface-bridge
// 図は docs/diagrams.md §18。

import AppKit
import ApplicationServices

let bridgeVersion = "1"

struct BridgeError: Error {
    let code: String
    let message: String
}

// MARK: - 出力（全部 main thread から呼ぶ）

func emit(_ obj: [String: Any]) {
    var data: Data
    if JSONSerialization.isValidJSONObject(obj), let d = try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys]) {
        data = d
    } else {
        data = Data("{\"ok\":false,\"error\":{\"code\":\"encode\",\"message\":\"応答を JSON にできません\"}}".utf8)
    }
    data.append(0x0a)
    FileHandle.standardOutput.write(data)
}

func nowMs() -> Double { (Date().timeIntervalSince1970 * 1000).rounded() }

func rectJSON(_ r: CGRect) -> [String: Any] {
    ["x": Double(r.origin.x), "y": Double(r.origin.y), "w": Double(r.size.width), "h": Double(r.size.height)]
}

func num(_ v: Any?) -> Double? {
    guard let n = v as? NSNumber else { return nil }
    let d = n.doubleValue
    return d.isFinite ? d : nil
}

func pidArg(_ req: [String: Any]) throws -> pid_t {
    guard let p = num(req["pid"]), p > 0, p < Double(Int32.max) else {
        throw BridgeError(code: "bad_args", message: "pid（正の整数）が要ります")
    }
    return pid_t(p)
}

// MARK: - AX の部品

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

func axFail(_ e: AXError, _ what: String) -> BridgeError {
    let hint: String
    switch e {
    case .apiDisabled: hint = "アクセシビリティの許可がありません"
    case .invalidUIElement: hint = "その窓はもう在りません"
    case .cannotComplete: hint = "app が応答しません（忙しい・dialog が開いている）"
    case .attributeUnsupported, .actionUnsupported: hint = "この app の窓はその操作に対応していません"
    default: hint = "AXError \(e.rawValue)"
    }
    return BridgeError(code: "ax_" + axName(e), message: "\(what): \(hint)")
}

func requireTrust() throws {
    if !AXIsProcessTrusted() {
        throw BridgeError(code: "not_trusted", message: "アクセシビリティの許可がありません（システム設定 › プライバシーとセキュリティ › アクセシビリティ）")
    }
}

// MARK: - window_list の締め切り（W12W-1c の反証: 3 属性と窓の欄を順に読むと 3 × timeout を超え、Rust の読み手が先に諦めた）

/// window_list の AX の読み（3 属性と窓の欄）が使ってよい時間の合計 = この数 × 今の ax_timeout（起動時か config）。
/// Rust の `surface::WINDOW_LIST_AX_READS` と同じ数（読み手はこれ + 2 秒待つ）。G31b: 前は起動時の値のままで、config で伸ばしても 3 × 3 秒で切れた
let windowListReads = 3.0
/// 締め切り（nowMs の値）。window_list の間だけ置く。nil の間は AX の読みを締め切らない（他の要求は前のまま）
var axDeadline: Double? = nil
/// 今の AX の timeout（秒・起動時と config で変わる）
var axTimeoutNow = 3.0
/// 残りがこれ未満なら読まない（秒）
let axMinRead = 0.05

/// 締め切りまでの残り（秒）。締め切りが無ければ nil
func axLeft() -> Double? { axDeadline.map { ($0 - nowMs()) / 1000 } }

/// 締め切りの中で el に AX の要求を 1 回: el の timeout を min(今の timeout, 残り) にし、終わったら global に戻す
/// （handle に残る窓の要素に短い timeout を残さない）。残りが無ければ要求せず nil
func withinDeadline<T>(_ el: AXUIElement, _ ask: () -> T) -> T? {
    guard let left = axLeft() else { return ask() }
    if left < axMinRead { return nil }
    AXUIElementSetMessagingTimeout(el, Float(min(axTimeoutNow, left)))
    defer { AXUIElementSetMessagingTimeout(el, 0) }
    return ask()
}

func axCopy(_ el: AXUIElement, _ attr: String) -> (CFTypeRef?, AXError) {
    withinDeadline(el) {
        var v: CFTypeRef?
        let e = AXUIElementCopyAttributeValue(el, attr as CFString, &v)
        return (v, e)
    } ?? (nil, .cannotComplete)
}

func axString(_ el: AXUIElement, _ attr: String) -> String? {
    let (v, e) = axCopy(el, attr)
    return e == .success ? v as? String : nil
}

func axBool(_ el: AXUIElement, _ attr: String) -> Bool? {
    let (v, e) = axCopy(el, attr)
    guard e == .success, let n = v as? NSNumber else { return nil }
    return n.boolValue
}

func axSettable(_ el: AXUIElement, _ attr: String) -> Bool {
    withinDeadline(el) {
        var settable: DarwinBoolean = false
        return AXUIElementIsAttributeSettable(el, attr as CFString, &settable) == .success && settable.boolValue
    } ?? false
}

func axFrame(_ el: AXUIElement) throws -> CGRect {
    let (pv, pe) = axCopy(el, kAXPositionAttribute)
    guard pe == .success, let pv, CFGetTypeID(pv) == AXValueGetTypeID() else { throw axFail(pe == .success ? .noValue : pe, "窓の位置を読めません") }
    let (sv, se) = axCopy(el, kAXSizeAttribute)
    guard se == .success, let sv, CFGetTypeID(sv) == AXValueGetTypeID() else { throw axFail(se == .success ? .noValue : se, "窓の大きさを読めません") }
    var p = CGPoint.zero
    var s = CGSize.zero
    guard AXValueGetValue(pv as! AXValue, .cgPoint, &p), AXValueGetValue(sv as! AXValue, .cgSize, &s) else {
        throw BridgeError(code: "ax_bad_value", message: "窓の位置 / 大きさの値の形が違います")
    }
    return CGRect(origin: p, size: s)
}

// MARK: - 窓の handle（bridge が生きている間だけ有効。AXUIElement を CFEqual で同定する）

var handles: [Int: (el: AXUIElement, pid: pid_t)] = [:]
var nextHandle = 1

func existingHandle(_ el: AXUIElement, _ pid: pid_t) -> Int? {
    handles.first { $0.value.pid == pid && CFEqual($0.value.el, el) }?.key
}

func handle(for el: AXUIElement, pid: pid_t) -> Int {
    if let h = existingHandle(el, pid) { return h }
    let h = nextHandle
    nextHandle += 1
    handles[h] = (el, pid)
    return h
}

func handleName(_ h: Int) -> String { "w\(h)" }

func element(_ req: [String: Any]) throws -> (AXUIElement, pid_t, Int) {
    guard let s = req["handle"] as? String, s.hasPrefix("w"), let h = Int(s.dropFirst()), let v = handles[h] else {
        throw BridgeError(code: "unknown_handle", message: "その窓の handle は知りません（windows で取り直してください）")
    }
    return (v.el, v.pid, h)
}

// MARK: - AX の要素の snapshot（ax_tree。ref = "e<snapshot>.<n>"・bridge が生きている間だけ）
// snapshot 番号は bridge 全体で 1 本の連番（ref だけで窓が決まる）。窓ごとに最新の 1 つだけを持ち、前の snapshot の ref は stale_element。

var nextSnapshot: UInt64 = 1
var axSnapshots: [Int: (snapshot: UInt64, elements: [AXUIElement])] = [:]

func refName(_ snapshot: UInt64, _ n: Int) -> String { "e\(snapshot).\(n)" }

/// label = AXTitle → AXDescription → 見出し（AXTitleUIElement の AXValue / AXTitle）の最初の空でない物
func axLabel(_ el: AXUIElement) -> String {
    for attr in [kAXTitleAttribute, kAXDescriptionAttribute] {
        if let s = axString(el, attr), !s.isEmpty { return s }
    }
    let (t, e) = axCopy(el, kAXTitleUIElementAttribute)
    if e == .success, let t, CFGetTypeID(t) == AXUIElementGetTypeID() {
        let te = t as! AXUIElement
        if let s = axString(te, kAXValueAttribute) ?? axString(te, kAXTitleAttribute), !s.isEmpty { return s }
    }
    return ""
}

/// AXValue を文字で（文字列と数だけ。それ以外の型は null）
func axValueText(_ el: AXUIElement) -> String? {
    let (v, e) = axCopy(el, kAXValueAttribute)
    guard e == .success, let v else { return nil }
    if let s = v as? String { return String(s.prefix(2000)) }
    if let n = v as? NSNumber { return n.stringValue }
    return nil
}

func axActions(_ el: AXUIElement) -> [String] {
    var names: CFArray?
    guard AXUIElementCopyActionNames(el, &names) == .success, let list = names as? [String] else { return [] }
    return list
}

/// Chromium / Electron の app（Discord 等）は、AT が来たと分かるまで web の中身を AX に出さない（G31 実測: bridge の読みだけでは 3 分たっても
/// 窓の枠の 12 要素・AXManualAccessibility を立てると 1〜2 秒で 300 要素以上）。pid ごとに 1 回だけ app 要素に AXManualAccessibility を立て、
/// 立った（success = Chromium）時だけ窓の木に AXWebArea が出るまで 1 回の要求で最長 1 秒待つ（実測は混んだ機械で 3 秒を超えた・Rust の待ちは
/// ax_timeout + 2 秒なので bridge では長く待たず、読み直しは Rust の `read` が持つ）。出ないまま立てて 15 秒以内なら true（= ax_tree の waking・呼ぶ側が読み直す）。
/// 出たら以後は見ない。Cocoa の app は attributeUnsupported を返すので待たない。
/// 画面・前面・app の中身は変えない（読み上げ用の木を作らせるだけ）。待つ間は Thread.sleep（main の queue を回すと次の要求が割り込む）
var manualAXTried: Set<pid_t> = []
var chromiumWokeAt: [pid_t: Date] = [:]
func wakeChromium(_ pid: pid_t, window: AXUIElement) -> Bool {
    if manualAXTried.insert(pid).inserted,
       AXUIElementSetAttributeValue(AXUIElementCreateApplication(pid), "AXManualAccessibility" as CFString, kCFBooleanTrue) == .success {
        chromiumWokeAt[pid] = Date()
    }
    guard let at = chromiumWokeAt[pid] else { return false }
    let giveUp = at.addingTimeInterval(15)
    let until = min(Date().addingTimeInterval(1), giveUp)
    while !hasWebArea(window) {
        if Date() >= until { return Date() < giveUp }
        Thread.sleep(forTimeInterval: 0.25)
    }
    chromiumWokeAt[pid] = nil
    return false
}
/// 窓の木の上の方（200 要素まで）に AXWebArea が在るか。app が答えない（cannot_complete）なら探し続けず false（1 回の要求を AX の timeout × 要素の数で止めない）
func hasWebArea(_ root: AXUIElement) -> Bool {
    var queue = [root]
    var n = 0
    while !queue.isEmpty && n < 200 {
        let cur = queue.removeFirst()
        n += 1
        let (role, re) = axCopy(cur, kAXRoleAttribute)
        if re == .cannotComplete { return false }
        if (role as? String) == "AXWebArea" { return true }
        let (kids, ke) = axCopy(cur, kAXChildrenAttribute)
        if ke == .cannotComplete { return false }
        queue += (kids as? [AXUIElement]) ?? []
    }
    return false
}

/// owner 256（「300要素で打ち切るとか打ち切ることはしないで」）: 読みかけの窓の木（handle ごとに 1 つ）。ax_tree は数で切らず、1 回の要求を
/// 時間の区切り（今の AX の timeout の 6 割）で返し、`more: true` の要求で同じ snapshot の続きを返す（bridge は要求を 1 本の列で捌くので、区切らないと
/// 読む間ほかの要求が通らず、呼ぶ側は「N 個まで読んだ」を出せない）。新しい ax_tree（more 無し）は前の読みかけを捨てる。止めるのは呼ぶ側（`MacGui::read`）
final class AxWalk {
    let snapshot: UInt64
    let actionableOnly: Bool
    let root: (role: String, label: String)?
    /// root を探す幅優先の列（[0] = 窓・root が無い・探し終えたら空）
    var look: [AXUIElement]
    var lookHead = 0
    var found: AXUIElement? = nil
    var looked = 0
    /// (要素, depth, 返した一番近い祖先の ref)。head より前は読み終えた物（removeFirst の O(n) を毎回払わない）
    var queue: [(AXUIElement, Int, String?)]
    var head = 0
    var kept: [AXUIElement] = []
    var visited = 0
    /// ENGINE-AUDIT W0（handover B・owner 256 の続き）: 列に入れた要素の集合（NSMutableSet = CF の isEqual / hash = 同じ pid と要素の id）。AXChildren が輪になる
    /// app でも列が終わる（数の上限ではない）。root を探す列（lookSeen）と本体の列（seen）は別に数える = 探した道の要素を本体で飛ばさない
    let seen = NSMutableSet()
    let lookSeen = NSMutableSet()
    init(window: AXUIElement, snapshot: UInt64, actionableOnly: Bool, root: (role: String, label: String)?) {
        self.snapshot = snapshot
        self.actionableOnly = actionableOnly
        self.root = root
        look = root == nil ? [] : [window]
        queue = [(window, 0, nil)]
        seen.add(window)
        lookSeen.add(window)
    }
    /// まだ列に入れていない要素だけ（入れた物は set に足す）
    static func fresh(_ els: [AXUIElement], _ set: NSMutableSet) -> [AXUIElement] {
        var out: [AXUIElement] = []
        for e in els where !set.contains(e) {
            set.add(e)
            out.append(e)
        }
        return out
    }
}
var axWalks: [Int: AxWalk] = [:]

/// ax_tree の本体: 窓から幅優先で辿る。elements[0] は窓そのもの。actionable_only の時は返さない要素を飛ばし、parent は返した一番近い祖先。
/// owner 256: 数では切らない。1 回の要求は時間の区切り（今の AX の timeout〈起動時か config・G31〉の 6 割）か app が応答しない（cannot_complete）で返し、
/// complete = false と paused（"time" = 区切り・"app" = 答えない）を付ける（error にしない）。答えなかった要素は列の頭に残し、次の more で読み直す。
/// elements はその要求で新しく読めた物だけ（ref は snapshot の中の通し番号のまま続く）。読み終えたら complete = true で walk を捨てる。
/// FX-112: root（role と label）= 窓から幅優先でその要素を探し（label は大文字小文字を問わず含む）、窓そのものとその要素から下だけを返す。
/// 見つからなければ窓だけ・root_found = false（探し終えるまでは root_found を付けない）
func axTree(_ w: AxWalk, handle h: Int) -> [String: Any] {
    let deadline = Date().addingTimeInterval(max(0.5, axTimeoutNow * 0.6))
    var out: [[String: Any]] = []
    var paused: String? = nil
    if let want = w.root {
        let label = want.label.lowercased()
        while w.found == nil && w.lookHead < w.look.count {
            if Date() > deadline { paused = "time"; break }
            let cur = w.look[w.lookHead]
            let (roleV, roleE) = axCopy(cur, kAXRoleAttribute)
            if roleE == .cannotComplete { paused = "app"; break }
            if w.lookHead > 0, (roleV as? String) == want.role, axLabel(cur).lowercased().contains(label) { w.found = cur; w.looked += 1; break }
            let (kids, ke) = axCopy(cur, kAXChildrenAttribute)
            if ke == .cannotComplete { paused = "app"; break }
            w.lookHead += 1
            w.looked += 1
            w.look += AxWalk.fresh((kids as? [AXUIElement]) ?? [], w.lookSeen)
        }
        if paused == nil { w.look = []; w.lookHead = 0 }
    }
    while paused == nil && w.head < w.queue.count {
        if Date() > deadline { paused = "time"; break }
        let (cur, depth, parent) = w.queue[w.head]
        let (roleV, roleE) = axCopy(cur, kAXRoleAttribute)
        if roleE == .cannotComplete { paused = "app"; break }
        let rooted = depth == 0 && w.root != nil
        var kids: [AXUIElement] = []
        if !rooted {
            let (k, e) = axCopy(cur, kAXChildrenAttribute)
            if e == .cannotComplete { paused = "app"; break }
            if e == .success { kids = (k as? [AXUIElement]) ?? [] }
        }
        w.head += 1
        w.visited += 1
        let role = (roleV as? String) ?? ""
        let actions = axActions(cur)
        let settable = axSettable(cur, kAXValueAttribute)
        let keep = depth == 0 || !w.actionableOnly || !actions.isEmpty || settable
        var myRef = parent
        if keep {
            let r = refName(w.snapshot, w.kept.count)
            myRef = r
            w.kept.append(cur)
            var o: [String: Any] = [
                "ref": r, "role": depth == 0 && role.isEmpty ? "AXWindow" : role,
                "subrole": axString(cur, kAXSubroleAttribute) ?? "", "label": axLabel(cur),
                "actions": actions, "value_settable": settable, "enabled": axBool(cur, kAXEnabledAttribute) ?? true,
                "depth": depth, "parent": parent ?? NSNull(),
            ]
            o["value"] = axValueText(cur) ?? NSNull()
            // ENGINE-AUDIT W3: 行・tab・項目の選ばれ・展開（属性の無い要素には入れない = Rust は None）
            if let s = axBool(cur, kAXSelectedAttribute) { o["selected"] = s }
            if let x = axBool(cur, kAXExpandedAttribute) { o["expanded"] = x }
            out.append(o)
        }
        if rooted {
            if let f = w.found, !w.seen.contains(f) {
                w.seen.add(f)
                w.queue.append((f, 1, myRef))
            }
        } else {
            w.queue.append(contentsOf: AxWalk.fresh(kids, w.seen).map { ($0, depth + 1, myRef) })
        }
    }
    axSnapshots[h] = (w.snapshot, w.kept)
    if paused == nil { axWalks[h] = nil }
    var tree: [String: Any] = ["handle": handleName(h), "snapshot": w.snapshot, "elements": out, "visited": w.visited + w.looked, "complete": paused == nil]
    if let p = paused { tree["paused"] = p }
    if w.root != nil, w.look.isEmpty { tree["root_found"] = w.found != nil }
    return tree
}

// MARK: - AX の操作（ax_press・ax_set_value・ax_menu・key_to_pid。YC W17-2・契約は src/adapters/mac_gui.rs の AxPressReq 等）
// error の code は Rust の AxErrorCode と同じ素の名前（ax_ を付けない）。cannot_complete だけが「送った後かもしれない」。

/// AXError → 操作の error（素の code）。それ以外の AXError は axFail（ax_ 付き・Rust は Unsupported に読む）
func opFail(_ e: AXError, _ what: String) -> BridgeError {
    switch e {
    case .cannotComplete: return BridgeError(code: "cannot_complete", message: "\(what): app が応答しません（送った後かもしれません）")
    case .invalidUIElement: return BridgeError(code: "invalid_element", message: "\(what): その要素はもう在りません")
    case .actionUnsupported, .attributeUnsupported: return BridgeError(code: "action_unsupported", message: "\(what): この要素はその操作に対応していません")
    case .apiDisabled: return BridgeError(code: "not_trusted", message: "\(what): アクセシビリティの許可がありません")
    default: return axFail(e, what)
    }
}

/// ref（"e<snapshot>.<n>"）→ ax_tree で返した要素。その窓に新しい snapshot があれば stale_element（読み直して ref を取り直す）
func elementRef(_ req: [String: Any]) throws -> AXUIElement {
    guard let s = req["element"] as? String, s.hasPrefix("e"), let dot = s.firstIndex(of: "."),
          let snap = UInt64(s[s.index(after: s.startIndex)..<dot]), let n = Int(s[s.index(after: dot)...]) else {
        throw BridgeError(code: "bad_args", message: "element（ax_tree の ref \"e<snapshot>.<n>\"）が要ります")
    }
    guard let hit = axSnapshots.values.first(where: { $0.snapshot == snap }) else {
        throw BridgeError(code: "stale_element", message: "要素 \(s) は古い読みの物です（ax_tree で読み直してください）")
    }
    guard n >= 0, n < hit.elements.count else { throw BridgeError(code: "invalid_element", message: "要素 \(s) はその読みに在りません") }
    return hit.elements[n]
}

func runningPid(_ req: [String: Any]) throws -> pid_t {
    let pid = try pidArg(req)
    guard NSRunningApplication(processIdentifier: pid) != nil else {
        throw BridgeError(code: "no_such_process", message: "pid \(pid) の app は動いていません")
    }
    return pid
}

/// 操作の応答は少し回してから返す（frontmost は通知で変わる。front_before ≠ front_after を Rust が D-g として数える・raise と同じ）
func replyAfterFront(_ id: Any, _ before: Int, _ result: [String: Any]) {
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
        var r = result
        r["front_before"] = before
        r["front_after"] = frontmostPid()
        emit(["id": id, "ok": true, "result": r])
    }
}

/// 値を置く（W17-2 戻し）。AXSelectedText と AXSelectedTextRange の両方を置ける要素は全選択 → AXSelectedText（app の編集として数わる =
/// TextEdit の書類が編集済みになり File › Save が disk に書く。AXValue の set は編集済みにならず保存しても disk が変わらなかった・2026-10-21）。
/// range か selected text の set が error なら AXValue に落ちる（AXValue は全体を置き換えるので、先の set が効いていても結果は同じ）。
/// G79: hit = 置き換える 1 か所（UTF-16 の range）と置き換えた後の全文。全選択せず、その range を選んで AXSelectedText。置いた range を読み直して
/// 違えば何も打たずに止める（app が選択を受け付けず、今の選択〈全文かもしれない〉に打つのを避ける）。AXValue に落ちる時は置き換えた後の全文を置く
func axPutText(_ el: AXUIElement, _ value: String, replacing hit: (range: CFRange, whole: String)? = nil) -> (AXError, String) {
    if axSettable(el, kAXSelectedTextRangeAttribute), axSettable(el, kAXSelectedTextAttribute) {
        var sel = hit?.range ?? CFRange(location: 0, length: (axCopy(el, kAXNumberOfCharactersAttribute).0 as? NSNumber)?.intValue
            ?? ((axCopy(el, kAXValueAttribute).0 as? String) ?? "").utf16.count)
        if let range = AXValueCreate(.cfRange, &sel),
           AXUIElementSetAttributeValue(el, kAXSelectedTextRangeAttribute as CFString, range) == .success {
            if let hit {
                var now = CFRange()
                guard let v = axCopy(el, kAXSelectedTextRangeAttribute).0, CFGetTypeID(v) == AXValueGetTypeID(),
                      AXValueGetValue(v as! AXValue, .cfRange, &now), now.location == hit.range.location, now.length == hit.range.length
                else { return (.failure, "replace_range") }
            }
            if AXUIElementSetAttributeValue(el, kAXSelectedTextAttribute as CFString, value as CFString) == .success {
                return (.success, "selected_text")
            }
        }
    }
    return (AXUIElementSetAttributeValue(el, kAXValueAttribute as CFString, (hit?.whole ?? value) as CFString), "value")
}

/// G79: needle が hay に完全一致する所（UTF-16 の range・重なりも数える）。ちょうど 1 つの時だけ置き換える
func exactRanges(_ needle: String, in hay: String) -> [CFRange] {
    let h = hay as NSString
    var out: [CFRange] = []
    var from = 0
    while from < h.length {
        let r = h.range(of: needle, options: .literal, range: NSRange(location: from, length: h.length - from))
        if r.location == NSNotFound { break }
        out.append(CFRange(location: r.location, length: r.length))
        from = r.location + 1
    }
    return out
}

/// menu bar を題名の完全一致で辿り、最後の項目だけを AXPress（途中の項目は押さない＝menu を画面に開かない）。
/// 返り値 = (押した題名の列, 押す前に読んだ最後の項目の AXEnabled)。背景の app の menu は検証され直さず AXEnabled が古い（TextEdit の Save は
/// 保存できる時も false・W17-2 戻し）ので、false でも止めずに押す（押した時に効くかは app が決める・呼ぶ側が読み戻す）
/// G31: 押す前の読み（menu bar・各階の子）の失敗は axFail（ax_ 付き = Rust は「送っていない」）。opFail の cannot_complete（送った後かもしれない）は
/// 最後の AXPress だけ（Zoom 実測: menu bar が読めずに何も押していないのに cannot_complete を返し、Rust が「送ったかもしれない・2 回打つな」と出していた。
/// 子の読みが黙って空になり「見つかりません（の下: ）」と出ていた）
func axMenuPress(_ pid: pid_t, _ path: [String]) throws -> ([String], Bool) {
    let (bar, e) = axCopy(AXUIElementCreateApplication(pid), kAXMenuBarAttribute)
    guard e == .success, let bar, CFGetTypeID(bar) == AXUIElementGetTypeID() else { throw axFail(e == .success ? .noValue : e, "menu bar を読めません") }
    var cur = bar as! AXUIElement
    var pressed: [String] = []
    var enabledBefore = true
    for (i, title) in path.enumerated() {
        // menu bar の項目・menu の項目の子は AXMenu 1 つ。その子が次の階の項目
        var items: [AXUIElement] = []
        let (kids, ke) = axCopy(cur, kAXChildrenAttribute)
        if ke == .cannotComplete { throw axFail(ke, "menu を読めません") }
        for k in (kids as? [AXUIElement]) ?? [] {
            if axString(k, kAXRoleAttribute) == (kAXMenuRole as String) {
                let (sub, se) = axCopy(k, kAXChildrenAttribute)
                if se == .cannotComplete { throw axFail(se, "menu を読めません") }
                items += (sub as? [AXUIElement]) ?? []
            } else {
                items.append(k)
            }
        }
        guard let hit = items.first(where: { axString($0, kAXTitleAttribute) == title }) else {
            let names = items.compactMap { axString($0, kAXTitleAttribute) }.filter { !$0.isEmpty }
            throw BridgeError(code: "menu_not_found", message: "「\(title)」が \(i + 1) 階目に見つかりません（\(pressed.joined(separator: " › "))の下: \(names.joined(separator: "・"))）")
        }
        if i == path.count - 1 {
            enabledBefore = axBool(hit, kAXEnabledAttribute) ?? true
            let pe = AXUIElementPerformAction(hit, kAXPressAction as CFString)
            guard pe == .success else { throw opFail(pe, "「\(title)」を押せません") }
        }
        pressed.append(title)
        cur = hit
    }
    return (pressed, enabledBefore)
}

/// ENGINE-AUDIT W3: menu bar の題名と 1 階目の項目の題名（読むだけ・押さない・前面を変えない・区切り = 題名の無い物は入れない・submenu は題名だけ）。
/// 契約は src/adapters/mac_gui.rs の AxMenuTreeReq / AxMenuTreeReply
func axMenuTree(_ pid: pid_t) throws -> [[String: Any]] {
    let (bar, e) = axCopy(AXUIElementCreateApplication(pid), kAXMenuBarAttribute)
    guard e == .success, let bar, CFGetTypeID(bar) == AXUIElementGetTypeID() else { throw axFail(e == .success ? .noValue : e, "menu bar を読めません") }
    let (tops, te) = axCopy(bar as! AXUIElement, kAXChildrenAttribute)
    if te == .cannotComplete { throw axFail(te, "menu を読めません") }
    var out: [[String: Any]] = []
    for top in (tops as? [AXUIElement]) ?? [] {
        guard let title = axString(top, kAXTitleAttribute), !title.isEmpty else { continue }
        var items: [String] = []
        // menu bar の項目の子は AXMenu 1 つ。その子が 1 階目の項目
        let (kids, ke) = axCopy(top, kAXChildrenAttribute)
        if ke == .cannotComplete { throw axFail(ke, "menu を読めません") }
        for k in (kids as? [AXUIElement]) ?? [] {
            let (sub, se) = axCopy(k, kAXChildrenAttribute)
            if se == .cannotComplete { throw axFail(se, "menu を読めません") }
            for it in (sub as? [AXUIElement]) ?? [] {
                if let t = axString(it, kAXTitleAttribute), !t.isEmpty { items.append(t) }
            }
        }
        out.append(["title": title, "items": items])
    }
    return out
}

/// key_to_pid の key 名（KangaeMirror.swift の keyCodes と同じ表）
let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15,
    "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31, "u": 32,
    "i": 34, "p": 35, "return": 36, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46, "tab": 48, "space": 49, "delete": 51, "escape": 53,
    "left": 123, "right": 124, "down": 125, "up": 126, "home": 115, "end": 119, "pageup": 116, "pagedown": 121, "forwarddelete": 117,
]

/// pid 宛ての key か文字（CGEvent.postToPid・前面と cursor を変えない・mouse は送らない）。返すのは送った event の数
func keyToPid(_ pid: pid_t, key: String?, mods: [String], text: String?) throws -> Int {
    let src = CGEventSource(stateID: .privateState)
    var sent = 0
    func post(_ e: CGEvent?) throws {
        guard let e else { throw BridgeError(code: "internal", message: "key の event を作れません") }
        e.postToPid(pid)
        sent += 1
    }
    if let text {
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
        return sent
    }
    guard let name = key?.lowercased(), let code = keyCodes[name] else { throw BridgeError(code: "bad_args", message: "知らない key: \(key ?? "")") }
    var flags: CGEventFlags = []
    for m in mods {
        switch m {
        case "cmd": flags.insert(.maskCommand)
        case "shift": flags.insert(.maskShift)
        case "option": flags.insert(.maskAlternate)
        case "control": flags.insert(.maskControl)
        default: throw BridgeError(code: "bad_args", message: "知らない修飾 key: \(m)")
        }
    }
    for down in [true, false] {
        let e = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down)
        e?.flags = flags
        try post(e)
    }
    return sent
}

// MARK: - CG の窓（前から後ろの順。bounds は AX と同じ左上原点の points）

func cgWindows(pid: pid_t?, onScreenOnly: Bool) -> [[String: Any]] {
    let opts: CGWindowListOption = onScreenOnly ? [.optionOnScreenOnly, .excludeDesktopElements] : [.optionAll, .excludeDesktopElements]
    guard let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] else { return [] }
    var out: [[String: Any]] = []
    for (i, w) in list.enumerated() {
        guard let owner = (w[kCGWindowOwnerPID as String] as? NSNumber)?.intValue else { continue }
        if let pid, owner != Int(pid) { continue }
        var o: [String: Any] = [
            "id": (w[kCGWindowNumber as String] as? NSNumber)?.intValue ?? 0,
            "pid": owner,
            "layer": (w[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0,
            "order": i,
            "on_screen": (w[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue ?? false,
        ]
        if let b = w[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: b as CFDictionary) {
            o["frame"] = rectJSON(r)
        }
        // 他の app の窓の題名は画面収録の許可が無いと入らない（無い時は欄ごと無い）
        if let n = w[kCGWindowName as String] as? String { o["title"] = n }
        if let n = w[kCGWindowOwnerName as String] as? String { o["owner"] = n }
        out.append(o)
    }
    return out
}

func sameFrame(_ any: Any?, _ r: CGRect) -> Bool {
    guard let f = any as? [String: Any], let x = num(f["x"]), let y = num(f["y"]), let w = num(f["w"]), let h = num(f["h"]) else { return false }
    return abs(x - r.origin.x) <= 1 && abs(y - r.origin.y) <= 1 && abs(w - r.size.width) <= 1 && abs(h - r.size.height) <= 1
}

func windowInfo(_ el: AXUIElement, pid: pid_t, cg: [[String: Any]]) -> [String: Any] {
    // 枠を先に読む: CG との対応（on_screen・今の Space）はこれで決まる。window_list の締め切りで後ろの欄が読めなくても残す
    let f = try? axFrame(el)
    var o: [String: Any] = [
        "handle": handleName(handle(for: el, pid: pid)),
        "pid": Int(pid),
        "title": axString(el, kAXTitleAttribute) ?? "",
        "role": axString(el, kAXRoleAttribute) ?? "",
        "subrole": axString(el, kAXSubroleAttribute) ?? "",
        "minimized": axBool(el, kAXMinimizedAttribute) ?? false,
        "main": axBool(el, kAXMainAttribute) ?? false,
        "focused": axBool(el, kAXFocusedAttribute) ?? false,
        "fullscreen": axBool(el, "AXFullScreen") ?? false,
        "position_settable": axSettable(el, kAXPositionAttribute),
        "size_settable": axSettable(el, kAXSizeAttribute),
        // FX-111: 窓の書類の file の URL（AXDocument・`file:///…`）。同じ題名の窓を Project の repo の書類で 1 つに決める（書類の無い窓は空）
        "document": axString(el, kAXDocumentAttribute) ?? "",
    ]
    if let f {
        o["frame"] = rectJSON(f)
        // 普通の窓（layer 0）を先に合わせ、無ければ他の layer の同じ枠（G31 実測: Zoom の設定の窓は layer 3 の浮く窓で、layer 0 だけ見ると
        // 窓番号が付かず AI の操作の一覧から落ちた）。どちらも同じ pid の窓だけ・枠が 1 つだけ一致する時だけ
        let same = cg.filter { sameFrame($0["frame"], f) }
        let base = same.filter { ($0["layer"] as? Int) == 0 }
        let hits = base.isEmpty ? same : base
        if hits.count == 1 {
            o["cg_id"] = hits[0]["id"]
            o["on_screen"] = hits[0]["on_screen"]
            o["cg_match"] = "frame"
        } else {
            o["cg_match"] = hits.isEmpty ? "none" : "ambiguous"
        }
    }
    return o
}

// MARK: - 観測（AXObserver。refcon は窓の handle 番号、app 自身の登録は 0）

var observers: [pid_t: AXObserver] = [:]

let axCallback: AXObserverCallback = { _, element, notification, refcon in
    let h = refcon.map { Int(bitPattern: $0) } ?? 0
    onAX(element, notification as String, h)
}

func onAX(_ el: AXUIElement, _ name: String, _ h: Int) {
    var pid: pid_t = 0
    if AXUIElementGetPid(el, &pid) != .success, let known = handles[h] { pid = known.pid }
    var o: [String: Any] = ["pid": Int(pid), "t": nowMs()]
    switch name {
    case kAXUIElementDestroyedNotification:
        let gone = h != 0 ? h : existingHandle(el, pid)
        o["event"] = "destroyed"
        if let gone {
            o["handle"] = handleName(gone)
            handles[gone] = nil
            axSnapshots[gone] = nil
            axWalks[gone] = nil
        }
        emit(o)
        return
    case kAXWindowCreatedNotification:
        o["event"] = "window_created"
        registerWindow(el, pid: pid)
    case kAXMovedNotification: o["event"] = "moved"
    case kAXResizedNotification: o["event"] = "resized"
    case kAXWindowMiniaturizedNotification: o["event"] = "minimized"
    case kAXWindowDeminiaturizedNotification: o["event"] = "deminimized"
    case kAXFocusedWindowChangedNotification, kAXMainWindowChangedNotification: o["event"] = "focused_window_changed"
    default:
        o["event"] = "ax"
        o["name"] = name
    }
    o["handle"] = handleName(h != 0 && handles[h] != nil ? h : handle(for: el, pid: pid))
    if let f = try? axFrame(el) { o["frame"] = rectJSON(f) }
    emit(o)
}

@discardableResult
func registerWindow(_ el: AXUIElement, pid: pid_t) -> [String] {
    guard let obs = observers[pid] else { return [] }
    let h = handle(for: el, pid: pid)
    var failed: [String] = []
    for name in [kAXMovedNotification, kAXResizedNotification, kAXUIElementDestroyedNotification, kAXWindowMiniaturizedNotification, kAXWindowDeminiaturizedNotification] {
        let e = AXObserverAddNotification(obs, el, name as CFString, UnsafeMutableRawPointer(bitPattern: h))
        if e != .success && e != .notificationAlreadyRegistered { failed.append("\(name):\(axName(e))") }
    }
    return failed
}

func axWindows(_ pid: pid_t) throws -> [AXUIElement] {
    let (v, e) = axCopy(AXUIElementCreateApplication(pid), kAXWindowsAttribute)
    guard e == .success else { throw axFail(e, "窓の一覧を読めません") }
    return (v as? [AXUIElement]) ?? []
}

// MARK: - NSWorkspace の通知（常に流す）

func appJSON(_ a: NSRunningApplication) -> [String: Any] {
    var o: [String: Any] = [
        "pid": Int(a.processIdentifier),
        "bundle_id": a.bundleIdentifier ?? "",
        "name": a.localizedName ?? "",
        "active": a.isActive,
        "hidden": a.isHidden,
        "finished_launching": a.isFinishedLaunching,
        "policy": a.activationPolicy == .regular ? "regular" : a.activationPolicy == .accessory ? "accessory" : "prohibited",
    ]
    if let d = a.launchDate { o["launched_at"] = d.timeIntervalSince1970 }
    if let u = a.bundleURL { o["path"] = u.path }
    return o
}

func watchWorkspace() {
    let nc = NSWorkspace.shared.notificationCenter
    let pairs: [(Notification.Name, String)] = [
        (NSWorkspace.didLaunchApplicationNotification, "app_launched"),
        (NSWorkspace.didTerminateApplicationNotification, "app_terminated"),
        (NSWorkspace.didActivateApplicationNotification, "app_activated"),
        (NSWorkspace.didHideApplicationNotification, "app_hidden"),
        (NSWorkspace.didUnhideApplicationNotification, "app_unhidden"),
    ]
    for (name, event) in pairs {
        nc.addObserver(forName: name, object: nil, queue: .main) { n in
            guard let a = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication else { return }
            var o = appJSON(a)
            o["event"] = event
            o["t"] = nowMs()
            if event == "app_terminated" {
                let pid = a.processIdentifier
                for (h, v) in handles where v.pid == pid { handles[h] = nil; axSnapshots[h] = nil; axWalks[h] = nil }
                if let obs = observers.removeValue(forKey: pid) {
                    CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(obs), .defaultMode)
                }
            }
            emit(o)
        }
    }
    nc.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { _ in
        emit(["event": "space_changed", "t": nowMs()])
    }
}

func activate(_ pid: pid_t) -> Bool {
    guard let app = NSRunningApplication(processIdentifier: pid) else { return false }
    if #available(macOS 14.0, *) {
        return app.activate()
    }
    return app.activate(options: [])
}

func frontmostPid() -> Int { Int(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0) }

// MARK: - 要求

/// 応答の result を返す。nil は「後で自分で応答する」（open_app・raise）。
func run(_ req: [String: Any], id: Any) throws -> [String: Any]? {
    guard let cmd = req["cmd"] as? String else { throw BridgeError(code: "bad_args", message: "cmd が要ります") }
    switch cmd {
    case "hello":
        return ["version": bridgeVersion, "pid": Int(getpid()), "trusted": AXIsProcessTrusted(),
                "os": ProcessInfo.processInfo.operatingSystemVersionString]

    case "config":
        // 混んだ機械では app の AX の応答が既定の 3 秒を超える（load 800 で REAPER が答えなかった）。秒数を変える
        guard let t = num(req["ax_timeout"]), t > 0, t <= 120 else { throw BridgeError(code: "bad_args", message: "ax_timeout（0〜120 秒）が要ります") }
        AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), Float(t))
        axTimeoutNow = t
        return ["ax_timeout": t]

    case "permission":
        // prompt = true は人が「許可」を押した時だけ（起動時に勝手に出さない）
        if (req["prompt"] as? Bool) == true {
            let opts = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
            return ["trusted": AXIsProcessTrustedWithOptions(opts), "prompted": true]
        }
        return ["trusted": AXIsProcessTrusted(), "prompted": false]

    case "open_settings":
        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") else {
            throw BridgeError(code: "internal", message: "設定の URL を作れません")
        }
        return ["opened": NSWorkspace.shared.open(url)]

    case "apps":
        let want = req["bundle_id"] as? String
        let regularOnly = (req["regular_only"] as? Bool) ?? true
        let apps = NSWorkspace.shared.runningApplications.filter {
            (want == nil || $0.bundleIdentifier == want) && (!regularOnly || $0.activationPolicy == .regular)
        }
        return ["apps": apps.map(appJSON), "frontmost": frontmostPid()]

    case "find_app":
        guard let bid = req["bundle_id"] as? String, !bid.isEmpty else { throw BridgeError(code: "bad_args", message: "bundle_id が要ります") }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bid) else {
            return ["installed": false, "bundle_id": bid]
        }
        let info = Bundle(url: url)?.infoDictionary
        return ["installed": true, "bundle_id": bid, "path": url.path,
                "version": info?["CFBundleShortVersionString"] as? String ?? "", "build": info?["CFBundleVersion"] as? String ?? ""]

    case "open_app":
        guard let bid = req["bundle_id"] as? String, let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bid) else {
            throw BridgeError(code: "not_installed", message: "その app はこの Mac に見つかりません")
        }
        let cfg = NSWorkspace.OpenConfiguration()
        cfg.activates = (req["activate"] as? Bool) ?? true
        NSWorkspace.shared.openApplication(at: url, configuration: cfg) { app, err in
            let pid = app.map { Int($0.processIdentifier) }
            let why = err?.localizedDescription
            DispatchQueue.main.async {
                if let pid {
                    emit(["id": id, "ok": true, "result": ["pid": pid, "bundle_id": bid]])
                } else {
                    emit(["id": id, "ok": false, "error": ["code": "open_failed", "message": why ?? "開けませんでした"]])
                }
            }
        }
        return nil

    case "app_unhide":
        // ENGINE-AUDIT W8a: 隠れた app の窓を見せる（unhide は activate しない = 前面は取らない・owner 254）。解けるまで 100 ms おきに最長 1.5 秒見て返す
        // （解けなくても error にしない = hidden: true・isHidden は main の run loop で更新される）
        let pid = try pidArg(req)
        guard let app = NSRunningApplication(processIdentifier: pid) else {
            throw BridgeError(code: "no_such_process", message: "pid \(pid) の app は動いていません")
        }
        let before = frontmostPid()
        if app.isHidden { _ = app.unhide() }
        func look(_ left: Int) {
            guard app.isHidden, left > 0 else {
                emit(["id": id, "ok": true, "result": ["hidden": app.isHidden, "front_before": before, "front_after": frontmostPid()]])
                return
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { look(left - 1) }
        }
        look(15)
        return nil

    case "open_document":
        // CHAT-013: 在る書類を bundle の app で開く（activate 既定 false = 前面を取らない頼み・TextEdit は前に出る事がある）。答え = 開いた app の pid
        guard let bid = req["bundle_id"] as? String, let path = req["path"] as? String, FileManager.default.fileExists(atPath: path) else {
            throw BridgeError(code: "bad_args", message: "bundle_id と在る file の path が要ります")
        }
        guard let appURL = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bid) else {
            throw BridgeError(code: "not_installed", message: "その app はこの Mac に見つかりません")
        }
        let cfg = NSWorkspace.OpenConfiguration()
        cfg.activates = (req["activate"] as? Bool) ?? false
        NSWorkspace.shared.open([URL(fileURLWithPath: path)], withApplicationAt: appURL, configuration: cfg) { app, err in
            let pid = app.map { Int($0.processIdentifier) }
            let why = err?.localizedDescription
            DispatchQueue.main.async {
                if let pid {
                    emit(["id": id, "ok": true, "result": ["pid": pid, "bundle_id": bid]])
                } else {
                    emit(["id": id, "ok": false, "error": ["code": "open_failed", "message": why ?? "開けませんでした"]])
                }
            }
        }
        return nil

    case "windows":
        try requireTrust()
        let pid = try pidArg(req)
        let cg = cgWindows(pid: pid, onScreenOnly: false)
        let list = try axWindows(pid)
        return ["pid": Int(pid), "windows": list.map { windowInfo($0, pid: pid, cg: cg) }, "cg_windows": cg]

    case "window_list":
        // 状態の読み（kangae-surface・yt-open・intel）用。CGWindowList は許可が要らないので必ず返し、AX が読めない時は throw せず ax_error に理由を書く
        // W12W-1c（Rust の読み手は src/audio/surface.rs の AxRead・ax_reads）: kAXWindows・kAXMainWindow・kAXFocusedWindow を別々に読み
        // （kAXWindows は別の Space の窓を落とし得る）、各々を ax_reads[] = {attr, code: success|ax_<axName>|not_trusted, count, elapsed_ms} に。
        // 窓は CFEqual で 1 つにまとめて windows[] に・ax_error は 3 つとも失敗した時だけ（空 = success の 0 個と no_value は失敗に数えない）
        // 読みは全部で windowListReads × 今の ax_timeout（budget_s が在ればその短い方）まで（各読みは残りで締め切る・残りが無い属性は skipped_deadline・窓の欄は空）
        let pid = try pidArg(req)
        let cg = cgWindows(pid: pid, onScreenOnly: false)
        var o: [String: Any] = ["pid": Int(pid), "cg_windows": cg, "windows": [] as [Any]]
        let attrs = [("windows", kAXWindowsAttribute), ("main_window", kAXMainWindowAttribute), ("focused_window", kAXFocusedWindowAttribute)]
        if !AXIsProcessTrusted() {
            o["ax_error"] = ["code": "not_trusted", "message": "アクセシビリティの許可がありません"]
            o["ax_reads"] = attrs.map { ["attr": $0.0, "code": "not_trusted", "count": 0, "elapsed_ms": 0] }
            return o
        }
        let app = AXUIElementCreateApplication(pid)
        // budget_s（任意・G31b）: 読み手がこれ以上待たない秒。3 × 今の上限より短ければこちらで打ち切る（答えない app の読み直しは 30 秒の上限で 1 回・
        // 読み手は 30 + 2 秒しか待たないので、3 × 30 秒まで読むと読み手が先に諦めて次の要求がこの読みの後ろに並ぶ）
        let total = windowListReads * axTimeoutNow
        axDeadline = nowMs() + min(total, num(req["budget_s"]).flatMap { $0 > 0 ? $0 : nil } ?? total) * 1000
        defer { axDeadline = nil }
        var found: [AXUIElement] = []
        var reads: [[String: Any]] = []
        var failures: [BridgeError] = []
        for (name, attr) in attrs {
            if let left = axLeft(), left < axMinRead {
                reads.append(["attr": name, "code": "skipped_deadline", "count": 0, "elapsed_ms": 0])
                failures.append(BridgeError(code: "skipped_deadline", message: "窓（\(name)）は読む時間が残っていません"))
                continue
            }
            let t0 = nowMs()
            let (v, e) = axCopy(app, attr)
            let ms = Int(nowMs() - t0)
            var got: [AXUIElement] = []
            if e == .success, let v {
                if let a = v as? [AXUIElement] { got = a } else if CFGetTypeID(v) == AXUIElementGetTypeID() { got = [v as! AXUIElement] }
            } else if e != .success && e != .noValue {
                failures.append(axFail(e, "窓（\(name)）を読めません"))
            }
            reads.append(["attr": name, "code": e == .success ? "success" : "ax_" + axName(e), "count": got.count, "elapsed_ms": ms])
            for w in got where !found.contains(where: { CFEqual($0, w) }) { found.append(w) }
        }
        o["ax_reads"] = reads
        o["windows"] = found.map { windowInfo($0, pid: pid, cg: cg) }
        if failures.count == attrs.count, let f = failures.first { o["ax_error"] = ["code": f.code, "message": f.message] }
        return o

    case "ax_find":
        // 窓の AX の子を幅優先で辿り、value / title / description に query を含む要素を返す（読むだけ。押さない・打たない）
        // owner 256: 数では切らない（窓を全部辿る）。最初の当たりで止める（呼ぶ側 src/audio/surface.rs の value_contains は在るかだけを見る）
        try requireTrust()
        let (el, _, h) = try element(req)
        guard let q = req["query"] as? String, !q.isEmpty else { throw BridgeError(code: "bad_args", message: "query が要ります") }
        var queue: [AXUIElement] = [el]
        var visited = 0
        var hits: [[String: Any]] = []
        while visited < queue.count && hits.isEmpty {
            let cur = queue[visited]
            visited += 1
            for attr in [kAXValueAttribute, kAXTitleAttribute, kAXDescriptionAttribute] {
                if let s = axString(cur, attr), s.contains(q) {
                    hits.append(["role": axString(cur, kAXRoleAttribute) ?? "", "attribute": attr, "value": String(s.prefix(300))])
                    break
                }
            }
            let (kids, e) = axCopy(cur, kAXChildrenAttribute)
            if e == .success, let list = kids as? [AXUIElement] { queue.append(contentsOf: list) }
        }
        return ["handle": handleName(h), "hits": hits, "visited": visited, "complete": visited >= queue.count]

    case "ax_text":
        // W16-2: 窓の文書の今の中身（保存していない編集も含む）。読むだけ（set・押す・前に出すをしない）。KangaeMirror.swift の ax-text と同じ読み:
        // 窓の AX の子を幅優先で辿った最初の AXTextArea の AXValue（NSAttributedString は string に）。見つからない・読めないは throw せず reason で返す
        try requireTrust()
        let (el, _, h) = try element(req)
        var queue: [(AXUIElement, Int)] = [(el, 0)]
        var visited = 0
        var area: AXUIElement?
        while !queue.isEmpty && visited < 2000 {
            let (cur, depth) = queue.removeFirst()
            visited += 1
            if axString(cur, kAXRoleAttribute) == kAXTextAreaRole { area = cur; break }
            let (kids, e) = axCopy(cur, kAXChildrenAttribute)
            if depth < 8, e == .success, let list = kids as? [AXUIElement] { queue += list.map { ($0, depth + 1) } }
        }
        guard let t = area else { return ["handle": handleName(h), "text": NSNull(), "reason": "ax_no_text_area"] }
        let (raw, err) = axCopy(t, kAXValueAttribute)
        guard err == .success, let v = raw else { return ["handle": handleName(h), "text": NSNull(), "reason": "ax_value_error_\(err.rawValue)"] }
        let text: String? = (v as? String) ?? (v as? NSAttributedString)?.string
        return ["handle": handleName(h), "text": text.map { $0 as Any } ?? NSNull(), "reason": text == nil ? "value_type_\(CFGetTypeID(v))" : "ok"]

    case "ax_tree":
        // 窓の AX の要素の一覧（読むだけ。押さない・打たない・前面を変えない）。契約は src/adapters/mac_gui.rs の AxTreeReq / AxTreeReply
        // Chromium の app は pid ごとの初回に AXManualAccessibility を立てて中身を待つ（wakeChromium・G31）
        // owner 256: 数で切らない。1 回の要求は時間で区切り、more = true で同じ読みの続き（AxWalk・axTree の doc）
        try requireTrust()
        let (el, pid, h) = try element(req)
        if let m = req["more"], !(m is Bool) { throw BridgeError(code: "bad_args", message: "more は true / false で") }
        if (req["more"] as? Bool) == true {
            guard let w = axWalks[h] else { throw BridgeError(code: "bad_args", message: "続きを読む木がありません（more の無い ax_tree から読み直してください）") }
            return axTree(w, handle: h)
        }
        if let a = req["actionable_only"], !(a is Bool) { throw BridgeError(code: "bad_args", message: "actionable_only は true / false で") }
        var root: (role: String, label: String)? = nil
        if let r = req["root"] {
            guard let o = r as? [String: Any], let role = o["role"] as? String, let label = o["label"] as? String, !role.isEmpty else {
                throw BridgeError(code: "bad_args", message: "root は {\"role\": 字, \"label\": 字} で")
            }
            root = (role, label)
        }
        let waking = wakeChromium(pid, window: el)
        let walk = AxWalk(window: el, snapshot: nextSnapshot, actionableOnly: (req["actionable_only"] as? Bool) ?? false, root: root)
        nextSnapshot += 1
        axWalks[h] = walk
        var tree = axTree(walk, handle: h)
        tree["waking"] = waking
        return tree

    case "ax_press":
        // ax_tree の要素 1 つに AX の action（既定 AXPress）。要素の actions に無い action は送らない。前面を変えない
        try requireTrust()
        let el = try elementRef(req)
        let action = (req["action"] as? String) ?? (kAXPressAction as String)
        // ENGINE-AUDIT W5（節 11「押し方が AXPress か行の 2 つしか無い」）: ladder = 梯子を 1 つ。自分 → 近い祖先（6 段まで）の action → 行（自分か近い祖先の AXRow）なら
        // 入れ物の AXSelectedRows → 行の AXSelected → 自分の AXShowMenu / AXExpand の順に試し、1 つ効いたら via にその段を書いて止める。前面は変えない（AX の action だけ）。
        // Mirror の ax_at_point（LAYOUT）と同じ梯子にする時はこの段の順を写す
        if (req["ladder"] as? Bool) == true {
            let before = frontmostPid()
            let parentOf: (AXUIElement) -> AXUIElement? = { c in
                let (p, pe) = axCopy(c, kAXParentAttribute)
                guard pe == .success, let p, CFGetTypeID(p) == AXUIElementGetTypeID() else { return nil }
                return (p as! AXUIElement)
            }
            // 1. 自分と近い祖先が action を持てば送る
            var cur: AXUIElement? = el
            var depth = 0
            while let c = cur, depth < 6 {
                if axActions(c).contains(action) {
                    let e = AXUIElementPerformAction(c, action as CFString)
                    guard e == .success else { throw opFail(e, "\(action) を送れません") }
                    let via = depth == 0 ? "self" : "ancestor:\(axString(c, kAXRoleAttribute) ?? "")"
                    replyAfterFront(id, before, ["performed": action, "via": via])
                    return nil
                }
                cur = parentOf(c)
                depth += 1
            }
            // 2. 行（自分か近い祖先の AXRow）= 入れ物の AXSelectedRows → 行の AXSelected
            cur = el
            depth = 0
            while let c = cur, depth < 6 {
                if axString(c, kAXRoleAttribute) == (kAXRowRole as String) {
                    if let table = parentOf(c), axSettable(table, kAXSelectedRowsAttribute),
                       AXUIElementSetAttributeValue(table, kAXSelectedRowsAttribute as CFString, [c] as CFArray) == .success {
                        replyAfterFront(id, before, ["performed": "AXSelectedRows", "via": "rows"])
                        return nil
                    }
                    if axSettable(c, kAXSelectedAttribute), AXUIElementSetAttributeValue(c, kAXSelectedAttribute as CFString, kCFBooleanTrue) == .success {
                        replyAfterFront(id, before, ["performed": "AXSelect", "via": "selected"])
                        return nil
                    }
                    break
                }
                cur = parentOf(c)
                depth += 1
            }
            // 3. 自分の AXShowMenu / AXExpand（隠れた項目を見せる）
            for alt in ["AXShowMenu", "AXExpand"] where axActions(el).contains(alt) {
                let e = AXUIElementPerformAction(el, alt as CFString)
                guard e == .success else { throw opFail(e, "\(alt) を送れません") }
                replyAfterFront(id, before, ["performed": alt, "via": alt == "AXShowMenu" ? "menu" : "expand"])
                return nil
            }
            throw BridgeError(code: "action_unsupported", message: "梯子のどの段でも押せません（自分と祖先に \(action) が無く、行の選択も menu も展開も無い）")
        }
        // OPEN-INPLACE-1101 の 3: AXPress を持たない一覧の行は選ぶ（AXSelected = true・Rust の run_desktop_matching が行の時だけ頼む）
        if action == "AXSelect" {
            guard axSettable(el, kAXSelectedAttribute) else {
                throw BridgeError(code: "action_unsupported", message: "この行は選べません（AXSelected を置けない）")
            }
            let before = frontmostPid()
            let e = AXUIElementSetAttributeValue(el, kAXSelectedAttribute as CFString, kCFBooleanTrue)
            guard e == .success else { throw opFail(e, "行を選べません") }
            replyAfterFront(id, before, ["performed": action])
            return nil
        }
        guard axActions(el).contains(action) else {
            throw BridgeError(code: "action_unsupported", message: "この要素は \(action) を持っていません（\(axActions(el).joined(separator: "・"))）")
        }
        let before = frontmostPid()
        // G74f: focus = true（Rust が menu の項目の AXPress に付ける）なら AXFocused=true を置いてから押す（Discord 18:45 の実測: 置いた後の AXPress で
        // 確認の dialog が出た・読み直した focused は false のまま）。置けなくても押す・rc は focus_rc で返す
        var r: [String: Any] = ["performed": action]
        if (req["focus"] as? Bool) == true {
            r["focus_rc"] = Int(AXUIElementSetAttributeValue(el, kAXFocusedAttribute as CFString, kCFBooleanTrue).rawValue)
        }
        let e = AXUIElementPerformAction(el, action as CFString)
        guard e == .success else { throw opFail(e, "\(action) を送れません") }
        replyAfterFront(id, before, r)
        return nil

    case "ax_set_value":
        // 値を置き（axPutText: 置ければ AXSelectedText・だめなら AXValue）、同じ要素の AXValue を読み戻す（matches = 置いた値と同じ）。置けない要素は送らない
        try requireTrust()
        let el = try elementRef(req)
        guard let value = req["value"] as? String else { throw BridgeError(code: "bad_args", message: "value（文字列）が要ります") }
        guard axSettable(el, kAXValueAttribute) else { throw BridgeError(code: "not_settable", message: "この要素の値は置けません") }
        let before = frontmostPid()
        let (e, via) = axPutText(el, value)
        guard e == .success else { throw opFail(e, "値を置けません") }
        let back = axValueText(el)
        replyAfterFront(id, before, ["set": true, "read_back": back ?? NSNull(), "matches": back == value, "via": via])
        return nil

    case "ax_replace_text":
        // G79: 欄の全文（切らない AXValue）の中で replace と完全一致する所がちょうど 1 つの時だけ、そこだけを value に置き換える（全選択しない）。
        // 0 か所・2 か所以上は何も置かない（ax_ 付き = Rust は「送っていない」）。matches = 置き換えた後の全文が、前の全文のその 1 か所を value にした物と同じ。
        // ax_set_value に足さず別の cmd にした: 前の bridge は知らない key を捨てて全文を置き換えるが、知らない cmd なら何も置かずに断る
        try requireTrust()
        let el = try elementRef(req)
        guard let value = req["value"] as? String, let old = req["replace"] as? String, !old.isEmpty else {
            throw BridgeError(code: "bad_args", message: "value と replace（空でない文字列）が要ります")
        }
        guard axSettable(el, kAXValueAttribute) else { throw BridgeError(code: "not_settable", message: "この要素の値は置けません") }
        guard let full = axCopy(el, kAXValueAttribute).0 as? String else { throw BridgeError(code: "ax_no_value", message: "欄の字を読めないので置き換えません") }
        let hits = exactRanges(old, in: full)
        guard hits.count == 1 else {
            throw BridgeError(code: "ax_replace_count", message: "置き換える字が欄に \(hits.count) か所あります（ちょうど 1 か所の時だけ置き換えます）。何も置いていません")
        }
        let whole = (full as NSString).replacingCharacters(in: NSRange(location: hits[0].location, length: hits[0].length), with: value)
        let before = frontmostPid()
        let (e, via) = axPutText(el, value, replacing: (hits[0], whole))
        if via == "replace_range" { throw BridgeError(code: "ax_replace_range", message: "置き換える所を app が選ばせないので何も打っていません") }
        guard e == .success else { throw opFail(e, "置き換えられません") }
        let after = axCopy(el, kAXValueAttribute).0 as? String
        replyAfterFront(id, before, ["set": true, "read_back": axValueText(el) ?? NSNull(), "matches": after == whole, "via": via,
                                     "length_before": full.utf16.count, "length_after": after.map { $0.utf16.count } ?? NSNull()])
        return nil

    case "ax_menu":
        // app の menu bar を題名の完全一致で辿り、最後の項目だけを押す（app を前に出さない・灰色と読めても押す = axMenuPress）
        try requireTrust()
        let pid = try runningPid(req)
        guard let path = req["path"] as? [String], !path.isEmpty, !path.contains(where: { $0.isEmpty }) else {
            throw BridgeError(code: "bad_args", message: "path（空でない題名の配列）が要ります")
        }
        let before = frontmostPid()
        let (pressed, enabledBefore) = try axMenuPress(pid, path)
        replyAfterFront(id, before, ["pressed": pressed, "enabled_before": enabledBefore])
        return nil

    case "ax_menu_tree":
        // ENGINE-AUDIT W3: app の menu bar を読むだけ（題名と 1 階目の項目・押さない・前面を変えない）。契約は src/adapters/mac_gui.rs の AxMenuTreeReq / AxMenuTreeReply
        try requireTrust()
        let pid = try runningPid(req)
        return ["menus": try axMenuTree(pid)]

    case "key_to_pid":
        // pid 宛ての key + mods か text のどちらか 1 つ（CGEvent.postToPid）。届いたかは返さない（Rust は常に未検証）
        try requireTrust()
        let pid = try runningPid(req)
        let key = req["key"] as? String
        let text = req["text"] as? String
        guard (key == nil) != (text == nil), !(key?.isEmpty ?? false), !(text?.isEmpty ?? false) else {
            throw BridgeError(code: "bad_args", message: "key か text のどちらか 1 つ（空でない）が要ります")
        }
        let mods = (req["mods"] as? [String]) ?? []
        let before = frontmostPid()
        let sent = try keyToPid(pid, key: key, mods: mods, text: text)
        replyAfterFront(id, before, ["sent": sent, "via": "pid"])
        return nil


    case "cg_windows":
        let pid = req["pid"] == nil ? nil : try pidArg(req)
        return ["windows": cgWindows(pid: pid, onScreenOnly: (req["on_screen_only"] as? Bool) ?? true), "frontmost": frontmostPid()]

    case "frame":
        try requireTrust()
        let (el, pid, h) = try element(req)
        var o = windowInfo(el, pid: pid, cg: cgWindows(pid: pid, onScreenOnly: false))
        o["handle"] = handleName(h)
        return o

    case "set_frame":
        try requireTrust()
        let (el, _, h) = try element(req)
        guard let x = num(req["x"]), let y = num(req["y"]), let w = num(req["w"]), let hh = num(req["h"]), w >= 1, hh >= 1 else {
            throw BridgeError(code: "bad_args", message: "x・y・w・h（w と h は 1 以上）が要ります")
        }
        if axBool(el, kAXMinimizedAttribute) == true { throw BridgeError(code: "minimized", message: "窓が最小化されています") }
        if axBool(el, "AXFullScreen") == true { throw BridgeError(code: "fullscreen", message: "窓が全画面です（全画面を解くと合わせられます）") }
        let target = CGRect(x: x, y: y, width: w, height: hh)
        var p = target.origin
        var s = target.size
        let e1 = AXUIElementSetAttributeValue(el, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &p)!)
        guard e1 == .success else { throw axFail(e1, "窓の位置を変えられません") }
        let e2 = AXUIElementSetAttributeValue(el, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &s)!)
        guard e2 == .success else { throw axFail(e2, "窓の大きさを変えられません") }
        // 大きさを変えると画面の端で位置を押し戻す app があるので、ずれていればもう一度位置を置く
        if let mid = try? axFrame(el), abs(mid.origin.x - p.x) > 0.5 || abs(mid.origin.y - p.y) > 0.5 {
            _ = AXUIElementSetAttributeValue(el, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &p)!)
        }
        let actual = try axFrame(el)
        return ["handle": handleName(h), "requested": rectJSON(target), "actual": rectJSON(actual), "t": nowMs()]

    case "minimize":
        // G91: Mirror の黄（しまう）で本物の窓も最小化する / 開く時に解く（kAXMinimizedAttribute・公開 API）
        try requireTrust()
        let (el, _, h) = try element(req)
        let want = (req["minimized"] as? Bool) ?? true
        let e = AXUIElementSetAttributeValue(el, kAXMinimizedAttribute as CFString, want ? kCFBooleanTrue : kCFBooleanFalse)
        guard e == .success else { throw axFail(e, want ? "窓を最小化できません" : "窓の最小化を解けません") }
        return ["handle": handleName(h), "minimized": axBool(el, kAXMinimizedAttribute) ?? want, "t": nowMs()]

    case "raise":
        try requireTrust()
        let (el, pid, h) = try element(req)
        let before = frontmostPid()
        let e = AXUIElementPerformAction(el, kAXRaiseAction as CFString)
        guard e == .success else { throw axFail(e, "窓を前に出せません") }
        let wantActivate = (req["activate"] as? Bool) == true
        let activated = wantActivate ? activate(pid) : false
        // frontmost は通知で変わるので、少し回してから読み戻す（focus を奪ったかを Rust が数える）
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) {
            emit(["id": id, "ok": true, "result": ["handle": handleName(h), "front_before": before, "front_after": frontmostPid(),
                                                    "activate_requested": wantActivate, "activate_accepted": activated, "t": nowMs()]])
        }
        return nil

    case "frontmost":
        let a = NSWorkspace.shared.frontmostApplication
        return ["pid": Int(a?.processIdentifier ?? 0), "bundle_id": a?.bundleIdentifier ?? ""]

    case "mouse":
        return ["buttons": NSEvent.pressedMouseButtons]

    case "observe":
        try requireTrust()
        let pid = try pidArg(req)
        if observers[pid] == nil {
            var created: AXObserver?
            let e = AXObserverCreate(pid, axCallback, &created)
            guard e == .success, let obs = created else { throw axFail(e, "観測を始められません") }
            CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(obs), .defaultMode)
            observers[pid] = obs
        }
        guard let obs = observers[pid] else { throw BridgeError(code: "internal", message: "観測を始められません") }
        var failed: [String] = []
        let app = AXUIElementCreateApplication(pid)
        for name in [kAXWindowCreatedNotification, kAXFocusedWindowChangedNotification, kAXMainWindowChangedNotification] {
            let e = AXObserverAddNotification(obs, app, name as CFString, nil)
            if e != .success && e != .notificationAlreadyRegistered { failed.append("\(name):\(axName(e))") }
        }
        let list = try axWindows(pid)
        for w in list { failed += registerWindow(w, pid: pid) }
        return ["observing": Int(pid), "windows": list.count, "failed": failed]

    case "unobserve":
        let pid = try pidArg(req)
        if let obs = observers.removeValue(forKey: pid) {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(obs), .defaultMode)
        }
        return ["observing": false]

    case "quit":
        emit(["id": id, "ok": true, "result": ["bye": true]])
        exit(0)

    default:
        throw BridgeError(code: "unknown_cmd", message: "知らない cmd: \(cmd)")
    }
}

func handleLine(_ line: String) {
    guard let obj = try? JSONSerialization.jsonObject(with: Data(line.utf8)), let req = obj as? [String: Any] else {
        emit(["id": NSNull(), "ok": false, "error": ["code": "bad_json", "message": "1 行 1 つの JSON object で送ってください"]])
        return
    }
    let id = req["id"] ?? NSNull()
    do {
        if let result = try run(req, id: id) { emit(["id": id, "ok": true, "result": result]) }
    } catch let e as BridgeError {
        emit(["id": id, "ok": false, "error": ["code": e.code, "message": e.message]])
    } catch {
        emit(["id": id, "ok": false, "error": ["code": "internal", "message": "\(error)"]])
    }
}

// MARK: - main

// Dock に出ない（prohibited）。NSWorkspace の通知を確実に受けるために NSApplication は作る。
// MIRROR-QUIET-1101: 作った時点で launchservicesd に check in するので、.app では Contents/Helpers に置き、自分の Info.plist
// （ai.shibubu.desktop.bridge・LSUIElement）を埋め込む（scripts/release/build-mac.sh）＝ Shibubu の bundle id で check in しない
NSApplication.shared.setActivationPolicy(.prohibited)
// 忙しい app の AX 呼び出しで固まらない（macOS の既定は 6 秒）。KANGAE_BRIDGE_AX_TIMEOUT（秒）か config で変える
let axTimeout = Float(ProcessInfo.processInfo.environment["KANGAE_BRIDGE_AX_TIMEOUT"] ?? "").flatMap { $0 > 0 && $0 <= 120 ? $0 : nil } ?? 3.0
AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), axTimeout)
axTimeoutNow = Double(axTimeout)
watchWorkspace()

let reader = Thread {
    while let line = readLine(strippingNewline: true) {
        if line.trimmingCharacters(in: .whitespaces).isEmpty { continue }
        DispatchQueue.main.async { handleLine(line) }
    }
    DispatchQueue.main.async { exit(0) }
}
reader.start()
emit(["event": "ready", "version": bridgeVersion, "pid": Int(getpid()), "trusted": AXIsProcessTrusted(), "ax_timeout": Double(axTimeout), "t": nowMs()])
RunLoop.main.run()
