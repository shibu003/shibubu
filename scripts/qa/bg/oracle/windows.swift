// oracle/windows — CGWindowListCopyWindowInfo を直接読む読み戻しの道（bridge の window_list も製品も通さない）。
// build: scripts/qa/bg/build.sh（../lib/Records.swift と一緒に compile して bin/windows へ）。許可: 画面収録（窓の名前だけ。bounds・z は無くても取れる）。
//
// 呼ぶ側の使い方:
//   bin/windows once [--pid <pid>]                                   > R/oracle/windows.before.json   # 1 枚（WindowsReading）
//   bin/windows sample --pid <pid> [--pid <pid> …] --out R/oracle/windows.jsonl --stop R/sample.stop [--interval-ms 200] &  # A1 の標本
// 保証する: 読むだけ（前面・窓の位置を変えない）。座標は global の point（左上原点・Retina の倍率なし）。z は on screen の窓を前から数えた順位。
//   sample は渡した pid の窓（off screen も z = -1 で）を全部の欄で、他の app の on screen の窓は owner と name を消して（pid・窓番号・bounds・z だけ）書く
//   = 編集した窓と Shibubu の窓の間に他の窓が挟まったかを見られる（A1 の「Shibubu の窓の直上」）・owner の書類の名前を evidence に残さない。pid を 2 つ以上渡すと行の pid は null。
// 保証しない: off screen の窓は once に含めるが z = -1。WebView は子の view なので窓として出ない（D1 の対象外）。

import CoreGraphics
import Foundation

@main
struct WindowsOracle {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        do {
            exit(try dispatch(args).rawValue)
        } catch let e as GuardError {
            FileHandle.standardError.write("windows: \(e)\n".data(using: .utf8)!)
            exit(e.exitCode.rawValue)
        } catch {
            FileHandle.standardError.write("windows: \(error)\n".data(using: .utf8)!)
            exit(ExitCode.usage.rawValue)
        }
    }

    /// 副命令: once | sample。
    static func dispatch(_ args: [String]) throws -> ExitCode {
        guard let sub = args.first else { throw GuardError.usage("once|sample") }
        let a = try Args(Array(args.dropFirst()))
        switch sub {
        case "once":
            try writeJSON(readOnce(pid: try a.pids("--pid").first), to: nil)
        case "sample":
            let pids = try a.pids("--pid")
            guard !pids.isEmpty else { throw GuardError.usage("sample --pid <pid> [--pid <pid> …] --out <path> --stop <path>") }
            let ms = Int(a.value("--interval-ms") ?? "200") ?? 0
            guard ms > 0 else { throw GuardError.usage("--interval-ms は正の整数") }
            try sample(pids: pids, out: URL(fileURLWithPath: try a.required("--out")), stop: URL(fileURLWithPath: try a.required("--stop")), intervalMs: ms)
        default:
            throw GuardError.usage("知らない副命令 \(sub)")
        }
        return .ok
    }
}

/// 1 枚読む。pid を渡せばその pid の窓だけ（z は全体の中の順位のまま）。off screen の窓も z = -1 で含める。
public func readOnce(pid: Int32?) -> WindowsReading {
    WindowsReading(schema: .windows, t: Date().timeIntervalSince1970, pid: pid, windows: readWindowList(pid: pid, includeOffscreen: true))
}

/// 標本 1 枚: `pids` の窓は全部の欄（off screen も）・他の app の窓は on screen の物だけ owner と name を消して。
public func sampleWindows(pids: [Int32]) -> [WindowInfo] {
    let want = Set(pids)
    return readWindowList(pid: nil, includeOffscreen: true).compactMap { w in
        if want.contains(w.pid) { return w }
        guard w.onscreen else { return nil }
        var o = w
        o.owner = nil
        o.name = nil
        return o
    }
}

/// stopfile が出来るまで interval ごとに標本（`sampleWindows`）を 1 行 1 件で追記する。
public func sample(pids: [Int32], out: URL, stop: URL, intervalMs: Int) throws {
    if !FileManager.default.fileExists(atPath: out.path) {
        guard FileManager.default.createFile(atPath: out.path, contents: nil) else { throw GuardError.badInput(path: out.path, reason: "作れない") }
    }
    let fh = try FileHandle(forWritingTo: out)
    fh.seekToEndOfFile()
    let enc = RecordJSON.encoder()
    while !FileManager.default.fileExists(atPath: stop.path) {
        var d = try enc.encode(WindowsReading(schema: .windows, t: Date().timeIntervalSince1970, pid: pids.count == 1 ? pids[0] : nil, windows: sampleWindows(pids: pids)))
        d.append(0x0A)
        fh.write(d)
        usleep(useconds_t(intervalMs * 1000))
    }
    try fh.close()
}
