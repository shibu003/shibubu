// 試験の側の記録の型（front-guard と oracle/windows が共有する・D-g の 5 つ + owner の窓の frame + 文字の漏れ）。
// 保存形式の正本は scripts/qa/bg/record.schema.json。この file の型はその JSON と 1 対 1（Codable・snake_case）。
// 使い方: build.sh が `swiftc -parse-as-library lib/Records.swift front-guard.swift -o bin/front-guard` のように一緒に compile する。
// 保証する: 座標は CGWindowList と同じ global の point（左上原点・Retina の倍率は掛けない）。時刻は unix 秒（小数）。
// 保証しない: 窓の名前（kCGWindowName）は画面収録の許可が無いと空。その時も bounds と z は取れる。

import CoreGraphics
import Foundation

/// 保存形式の版。JSON の `schema` 欄に入れる。形を変えたら版を上げ、tally.py は知らない版を読まず止まる。
public enum SchemaId: String, Codable {
    case snapshot = "shibubu.qa.snapshot/1"
    case event = "shibubu.qa.event/1"
    case verdict = "shibubu.qa.verdict/1"
    case windows = "shibubu.qa.windows/1"
    case axprobe = "shibubu.qa.axprobe/1"
    case axWindows = "shibubu.qa.axwindows/1"
    case axWindowsVerdict = "shibubu.qa.axwindows-verdict/1"
}

/// D-g の 5 つ + この列で足す 2 つ（owner の窓の frame・文字の漏れ）。番号は YC_WAVES §1 D-g の番号。
/// 使い方: verdict の `measured` / `unmeasured` の鍵。保証: 7 つ以外の値は JSON に出さない。
public enum Detection: String, Codable, CaseIterable, CodingKeyRepresentable {
    /// 1: 前面の app と cursor の位置（手の前後の `lsappinfo front` 相当・`CGEventGetLocation`）
    case front
    /// 2: 手の途中の前面化（run の間ずっと `NSWorkspaceDidActivateApplicationNotification`）
    case activation
    /// 3: 窓の重なり（owner の前面の app の窓と交わる位置で、その上に疑いの pid の窓が来た）
    case zorder
    /// 4: Space の変化（`NSWorkspaceActiveSpaceDidChangeNotification`）
    case space
    /// 5: keyboard focus（system-wide の `AXFocusedUIElement` が手の前後で同じ）
    case focus
    /// owner の前面の app の窓の frame（bounds）が手の前後で同じ
    case ownerFrame = "owner_frame"
    /// 打った文字（needle）が owner の focus した element の値に現れた
    case leak
}

public struct Point: Codable, Equatable { public var x: Double; public var y: Double }
public struct Rect: Codable, Equatable { public var x: Double; public var y: Double; public var w: Double; public var h: Double }

/// app の同一性は pid で見る（bundle_id・name は人が読むため）。
public struct AppRef: Codable, Equatable {
    public var pid: Int32
    public var bundleId: String?
    public var name: String?
}

/// CGWindowListCopyWindowInfo の 1 行。`z` は前から数えた順位（0 が一番前・on screen の窓だけで数える）。
public struct WindowInfo: Codable, Equatable {
    public var windowId: UInt32
    public var pid: Int32
    public var owner: String?
    public var name: String?
    public var layer: Int
    public var bounds: Rect
    public var onscreen: Bool
    public var z: Int
}

/// system-wide の AXFocusedUIElement。AXUIElement は snapshot をまたいで比べられないので、この欄の組で同一を見る。
/// 保証しない: 同じ role・identifier・frame の別の element は区別できない（ponytail: 足りなければ AXParent の鎖の hash を足す）。
public struct FocusRef: Codable, Equatable {
    public var pid: Int32
    public var role: String?
    public var subrole: String?
    public var identifier: String?
    public var frame: Rect?
    /// `snap --needle` を渡した時だけ: その element の AXValue に needle が含まれたか。値そのものは記録しない（owner の文字を残さない）。
    public var needlePresent: Bool?
}

/// 測れなかった理由。理由の付いた検出は verdict で「未測」になり、0 件の緑に数えない。
public enum Unmeasured: String, Codable, CodingKeyRepresentable {
    case accessibilityDenied = "accessibility_denied"
    case screenRecordingDenied = "screen_recording_denied"
    case noOwnerFront = "no_owner_front"
    case noNeedle = "no_needle"
    case watcherNotRunning = "watcher_not_running"
    /// system-wide の AXFocusedUIElement が手の前後とも無い（Electron の窓など）。nil == nil を「同じ」の緑にしない
    case noFocusElement = "no_focus_element"
}

/// 手の対象の app 1 つの見え方（W15-b70）。`snap --pid` の pid ごと。窓の名前・本文は持たない（数だけ）。
public struct TargetState: Codable, Equatable {
    public var pid: Int32
    public var name: String?
    /// NSRunningApplication.isHidden（⌘H）。hidden の間は AX の手が rc 0 でも何も起きない
    public var hidden: Bool
    /// CGWindowList の on screen・layer 0・この pid の窓の数
    public var onscreenWindows: Int
}

/// 手の前か後の 1 枚。`front-guard snap` が書く。
public struct Snapshot: Codable {
    public var schema: SchemaId
    public var t: Double
    public var front: AppRef?
    public var cursor: Point
    public var focus: FocusRef?
    public var windows: [WindowInfo]
    /// この snapshot で取れなかった検出と理由（例: Accessibility が無い → focus と leak）
    public var unmeasured: [Detection: Unmeasured]
    /// `snap --pid` を渡した時だけ。起きていない pid は入れない（落ちた app を hidden と読まない）
    public var targets: [TargetState]?
}

/// `front-guard watch` が events.jsonl に 1 行 1 件で書く。
public struct Event: Codable {
    public enum Kind: String, Codable { case start, stop, activated, spaceChanged = "space_changed", zSample = "z_sample" }
    public var schema: SchemaId
    public var t: Double
    public var kind: Kind
    /// kind = activated の時: 前面になった app
    public var app: AppRef?
    /// kind = z_sample の時: その時点の on screen の窓（z 順）
    public var windows: [WindowInfo]?
}

public struct Violation: Codable {
    public var detection: Detection
    public var t: Double
    /// 人が読む 1 行（例: "front 412(Orca) -> 9031(Shibubu)"）
    public var detail: String
}

/// `front-guard judge` の出力。`ok` は「violations が 0 かつ unmeasured が 0」の時だけ true。
public struct Verdict: Codable {
    public var schema: SchemaId
    public var step: String
    public var ok: Bool
    public var violations: [Violation]
    public var measured: [Detection]
    public var unmeasured: [Detection: Unmeasured]
}

/// oracle/windows が書く。D1・D2・L4・A1 の標本の正（bridge の window_list は使わない）。
public struct WindowsReading: Codable {
    public var schema: SchemaId
    public var t: Double
    public var pid: Int32?
    public var windows: [WindowInfo]
}

// MARK: - W12W-1c: 1 手ごとの AX の切り分け（bin/axprobe が書く・tally.py の cause_of が読む）

/// AX の 1 属性の答え。空（答えて 0）・答えない（AXError）・timeout・許可無しを混ぜない（owner 13 通目）。
public enum AxOutcome: String, Codable {
    /// 答えて 1 個以上
    case answered
    /// 答えて 0 個（success で空の配列・no_value）
    case empty
    /// AXError（timeout と許可無し以外。すぐ返った cannot_complete = AX の口が無い・GUI でない pid もここ）
    case error
    /// cannot_complete で --timeout-ms の 9 割以上待った（app が答えない）
    case timeout
    /// AXIsProcessTrusted が false・api_disabled
    case notTrusted = "not_trusted"
}

/// AX の 1 属性を 1 回読んだ結果。axCode は AXError の名前（success・cannot_complete・no_value…・bridge の axName と同じ綴り）。
public struct AxAttempt: Codable, Equatable {
    public var outcome: AxOutcome
    public var axCode: String
    public var elapsedMs: Double
    /// 窓の数（main_window・focused_window は 0 か 1）
    public var count: Int
}

/// 窓を取る 3 つの属性を別々に（`kAXWindowsAttribute` は別の Space の窓を落とし得る・AltTab も一覧だけに頼らない）。
public struct AxAttempts: Codable, Equatable {
    public var windows: AxAttempt
    public var mainWindow: AxAttempt
    public var focusedWindow: AxAttempt
}

/// --want の AX identifier（末尾 `*` は前方一致・`role=<AXRole>` は role）を、3 つの属性で取れた窓の部分木から幅優先で探した結果（読むだけ・押さない）。
public struct WantProbe: Codable, Equatable {
    public var identifier: String
    public var found: Bool
    public var visited: Int
    /// 上限（max elements か --timeout-ms）の前に木を全部見た。false で found も false なら「欠けた」とは言えない（未測）
    public var complete: Bool
    public var elapsedMs: Double
}

/// 今の（前面の）Space の種類。公開 API（CG の窓の bounds と画面の大きさ）で決められなければ unknown（推し量らない）。
public enum SpaceKind: String, Codable {
    case normal
    case fullscreen
    case splitView = "split_view"
    case unknown
}

/// kern.memorystatus_vm_pressure_level（1 / 2 / 4）。読めなければ unknown。
public enum MemoryPressure: String, Codable {
    case normal, warn, critical, unknown
}

/// 混雑の生の数（判定はしない・閾値は tally.py の Thresholds。load と ncpu は記録だけで閾値にしない）。
public struct Pressure: Codable, Equatable {
    public var load1: Double
    public var ncpu: Int
    public var memoryPressure: MemoryPressure
    /// vm.swapusage の used（MB）。読めなければ nil
    public var swapUsedMb: Double?
    /// IOHIDSystem の HIDIdleTime（秒）。120 未満 = owner が PC を使っている（全画面 / Split View の場を作らない）
    public var hidIdleS: Double?
    /// pointer の最後の入力（mouseMoved / left・rightMouseDown / scrollWheel）からの秒の最小。cua-driver は pointer を打たないので自分の手で戻らない（W15-b42）
    public var mouseIdleS: Double?
    /// 最後の keyDown からの秒。cua-driver の type_text も戻す → 自分の post の時刻を owner_idle.py が除く
    public var keyIdleS: Double?
}

/// `bin/axprobe` の出力（1 手に 1 つ・run.sh step が steps/<name>.axprobe.json に書く）。
/// 保証: pid は run.json の pid（試験が起こした Shibubu）だけ・他の app の AX は読まない・前面も cursor も変えない。
/// 保証しない: 操作できる事（cg に窓が在っても operable にしない・tally.py の cause_of が want で決める）。
public struct AxProbe: Codable {
    public var schema: SchemaId
    public var t: Double
    public var step: String
    public var pid: Int32
    public var trusted: Bool
    public var ax: AxAttempts
    public var want: WantProbe?
    /// その pid の CG の窓（off screen も・readWindowList(pid:includeOffscreen: true)）
    public var cg: [WindowInfo]
    public var space: SpaceKind
    public var pressure: Pressure
    /// その pid の scheduling priority（`ps -o pri`・proc_pidinfo の pti_priority）。背景の App Nap は 4 の帯。読めなければ nil（記録だけ・判定に使わない）
    public var pri: Int?
}

/// W2b-AX: 外から見た Shibubu の `kAXWindows` の 1 回の読み（front-guard ax-windows / ax-sample が書く）。
/// 契約「CGWindowList で窓が onscreen なら AX の窓が常に 1 個以上在る」を外から測る道（製品の自己申告は使わない）。
/// 使い方: `front-guard ax-windows --pid <pid> --variant head > R/oracle/<step>.after.axwindows.json`。
/// 保証する: 応答しない（a: main thread が AX に答えない）と、答えたが 0 個（b・c: 窓が AX から外れた）を `state` で分ける。
/// 保証しない: 原因そのもの（どの仮説かは variant を替えた run の率の比で決める）。
public enum AxWindowsState: String, Codable, CaseIterable {
    /// 答えて 1 個以上
    case attached
    /// 答えたが 0 個（err = 0・cg_onscreen > 0）＝ 仮説 b / c
    case detached
    /// kAXErrorCannotComplete（-25204）＝ 時間内に答えない ＝ 仮説 a
    case unresponsive
    /// それ以外の AXError（-25211 apiDisabled 等）
    case error
    /// CGWindowList に onscreen の窓が無い（契約の対象外）
    case noWindow = "no_window"
}

public struct AxWindowsReading: Codable {
    public var schema: SchemaId
    public var t: Double
    public var pid: Int32
    /// 比べる build の名前（例 head / no_orderback）。run.sh が渡す・無ければ null
    public var variant: String?
    /// kAXWindows の個数。答えなかった時は null
    public var count: Int?
    /// AXUIElementCopyAttributeValue の AXError の生の値（0 = success）
    public var err: Int32
    /// 読みにかかった秒
    public var secs: Double
    /// 同じ時刻の CGWindowList の layer 0・onscreen の窓の数（同じ pid）
    public var cgOnscreen: Int
    /// 1 分の load（混雑下の赤を環境由来と分けるため）
    public var load1: Double?
    public var state: AxWindowsState

    enum CodingKeys: String, CodingKey { case schema, t, pid, variant, count, err, secs, cgOnscreen, load1, state }
    /// 無い値も null で書く（record.schema.json の required）。
    public func encode(to e: Encoder) throws {
        var c = e.container(keyedBy: CodingKeys.self)
        try c.encode(schema, forKey: .schema); try c.encode(t, forKey: .t); try c.encode(pid, forKey: .pid)
        try c.encode(variant, forKey: .variant); try c.encode(count, forKey: .count); try c.encode(err, forKey: .err)
        try c.encode(secs, forKey: .secs); try c.encode(cgOnscreen, forKey: .cgOnscreen); try c.encode(load1, forKey: .load1)
        try c.encode(state, forKey: .state)
    }
}

/// ax-judge の結果。exit: 0 detached も unresponsive も 0 件 / 1 detached が 1 件以上 / 6 unresponsive だけ（測れない＝緑にしない）。
public struct AxWindowsVerdict: Codable {
    public var schema: SchemaId
    public var samples: Int
    public var byState: [String: Int]
    /// 最初の detached の時刻（無ければ null）
    public var firstDetached: Double?
    public var ok: Bool

    enum CodingKeys: String, CodingKey { case schema, samples, byState, firstDetached, ok }
    public func encode(to e: Encoder) throws {
        var c = e.container(keyedBy: CodingKeys.self)
        try c.encode(schema, forKey: .schema); try c.encode(samples, forKey: .samples); try c.encode(byState, forKey: .byState)
        try c.encode(firstDetached, forKey: .firstDetached); try c.encode(ok, forKey: .ok)
    }
}

/// count・err・cg_onscreen から state を決める純関数（ax-windows と ax-judge が同じ物を使う）。
/// 順: cg_onscreen = 0 → noWindow / err = -25204 → unresponsive / err ≠ 0 → error / count = 0 → detached / それ以外 attached。
public func classifyAxWindows(count: Int?, err: Int32, cgOnscreen: Int) -> AxWindowsState {
    if cgOnscreen == 0 { return .noWindow }
    if err == -25204 { return .unresponsive }  // kAXErrorCannotComplete
    if err != 0 { return .error }
    if count == 0 { return .detached }
    return .attached
}

/// 道具の失敗。exit code は ExitCode に揃える。
public enum GuardError: Error {
    case usage(String)
    case permission(Unmeasured)
    case badInput(path: String, reason: String)
    /// 前面を奪う負の対照を FG 枠の印（--fg-window）無しで呼んだ
    case fgWindowRequired

    public var exitCode: ExitCode {
        switch self {
        case .usage: return .usage
        case .permission: return .permission
        case .badInput: return .badInput
        case .fgWindowRequired: return .fgWindowRequired
        }
    }
}

/// 道具の exit code（run.sh はこれで止まる・止まらないを決める）。
public enum ExitCode: Int32 {
    case ok = 0
    /// 1 つでも violation（run を失敗にする）
    case violation = 1
    case usage = 2
    case permission = 3
    case badInput = 4
    case fgWindowRequired = 5
    /// violation は 0 だが未測の検出がある（緑にしない）
    case unmeasured = 6
    /// 本体が未実装（interface の段の stub）。緑と読ませない
    case unimplemented = 70
}

/// snake_case の JSON の encoder / decoder（全道具で同じ物を使う）。
public enum RecordJSON {
    public static func encoder() -> JSONEncoder {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        e.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return e
    }
    public static func decoder() -> JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }
}

/// interface の段の本体。呼ばれたら exit 70 で止まる（stub を緑と読ませない）。
public func unimplemented(_ what: String) -> Never {
    FileHandle.standardError.write("unimplemented: \(what)\n".data(using: .utf8)!)
    exit(ExitCode.unimplemented.rawValue)
}

// MARK: - 共有の読み方（front-guard と oracle/windows が同じ物を使う）

/// CGWindowListCopyWindowInfo を前から読む。`z` は on screen の窓を前から数えた順位。
/// includeOffscreen = true の時は off screen の窓も足す（z = -1）。pid を渡せばその pid の窓だけ（z は全体の中の順位のまま）。
public func readWindowList(pid: Int32? = nil, includeOffscreen: Bool = false) -> [WindowInfo] {
    func rows(_ opt: CGWindowListOption) -> [[String: Any]] {
        (CGWindowListCopyWindowInfo(opt, kCGNullWindowID) as? [[String: Any]]) ?? []
    }
    func info(_ d: [String: Any], z: Int) -> WindowInfo? {
        guard let id = d[kCGWindowNumber as String] as? UInt32, let p = d[kCGWindowOwnerPID as String] as? Int32,
              let b = d[kCGWindowBounds as String] as? NSDictionary, let r = CGRect(dictionaryRepresentation: b as CFDictionary) else { return nil }
        return WindowInfo(windowId: id, pid: p, owner: d[kCGWindowOwnerName as String] as? String, name: d[kCGWindowName as String] as? String,
                          layer: d[kCGWindowLayer as String] as? Int ?? 0, bounds: Rect(x: r.minX, y: r.minY, w: r.width, h: r.height),
                          onscreen: z >= 0, z: z)
    }
    var out = rows([.optionOnScreenOnly]).enumerated().compactMap { info($0.element, z: $0.offset) }
    if includeOffscreen {
        let seen = Set(out.map(\.windowId))
        out += rows([.optionAll]).compactMap { info($0, z: -1) }.filter { !seen.contains($0.windowId) }
    }
    return pid.map { p in out.filter { $0.pid == p } } ?? out
}

public extension Rect {
    /// 面積が正の交わりがあるか（辺が接するだけは数えない）。
    func intersects(_ o: Rect) -> Bool {
        min(x + w, o.x + o.w) - max(x, o.x) > 0 && min(y + h, o.y + o.h) - max(y, o.y) > 0
    }
}

/// `--name value` の形の引数を読む（flags に挙げた名前だけは値を取らない）。知らない形は GuardError.usage。
public struct Args {
    public var positional: [String] = []
    var opts: [String: [String]] = [:]
    var flagsSeen: Set<String> = []
    public init(_ args: [String], flags: Set<String> = []) throws {
        var i = 0
        while i < args.count {
            let a = args[i]
            if a.hasPrefix("--") {
                if flags.contains(a) { flagsSeen.insert(a); i += 1; continue }
                guard i + 1 < args.count else { throw GuardError.usage("\(a) に値が要る") }
                opts[a, default: []].append(args[i + 1]); i += 2
            } else { positional.append(a); i += 1 }
        }
    }
    public func value(_ n: String) -> String? { opts[n]?.last }
    public func values(_ n: String) -> [String] { opts[n] ?? [] }
    public func flag(_ n: String) -> Bool { flagsSeen.contains(n) }
    public func required(_ n: String) throws -> String {
        guard let v = value(n) else { throw GuardError.usage("\(n) が要る") }
        return v
    }
    public func pids(_ n: String) throws -> [Int32] {
        try values(n).map { guard let p = Int32($0) else { throw GuardError.usage("\(n) \($0) は pid でない") }; return p }
    }
}

/// 1 つの JSON を stdout か file に書く（file は丸ごと置き換え）。
public func writeJSON<T: Encodable>(_ v: T, to path: String?) throws {
    var data = try RecordJSON.encoder().encode(v)
    data.append(0x0A)
    if let path { try data.write(to: URL(fileURLWithPath: path), options: .atomic) } else { FileHandle.standardOutput.write(data) }
}

public func readJSON<T: Decodable>(_ t: T.Type, _ path: String) throws -> T {
    do { return try RecordJSON.decoder().decode(t, from: Data(contentsOf: URL(fileURLWithPath: path))) }
    catch { throw GuardError.badInput(path: path, reason: "\(error)") }
}
