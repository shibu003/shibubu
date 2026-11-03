// wincap — 試験の側の道具（製品の依存ではない）。窓 1 つを ScreenCaptureKit で PNG に撮る（W12W-1 の probe 5）。
// WKWebView の中身は `screencapture -l` に写らない（report-5）ので、SCScreenshotManager に desktopIndependentWindow で窓を渡す。
// 前面・cursor・窓の位置は変えない（撮るだけ）。後ろに隠れた窓も撮れる見込み（未測の時は撮れた PNG を見て決める）。
//
// 使い方: wincap --wid <CGWindowID> --out <png>
// 保証する: 撮れたら PNG を書いて exit 0・1 行目に "ok <w>x<h> <out>"。窓が無い・許可が無い・撮れないは exit 1 と理由 1 行（stderr）。
// 保証しない: 写った中身が正しい事（PNG を見るのは呼ぶ側）。画面収録の許可を与える事（owner の手）。
import AppKit
import CoreGraphics
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

@main
struct WinCap {
    static func main() async {
        _ = NSApplication.shared // CG の接続を作る（無いと CGS_REQUIRE_INIT で落ちる）
        let args = CommandLine.arguments
        func arg(_ k: String) -> String? { args.firstIndex(of: k).flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil } }
        guard let w = arg("--wid").flatMap(UInt32.init), let out = arg("--out") else {
            FileHandle.standardError.write("wincap: 使い方: wincap --wid <CGWindowID> --out <png>\n".data(using: .utf8)!)
            exit(2)
        }
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
            guard let win = content.windows.first(where: { $0.windowID == w }) else {
                throw NSError(domain: "wincap", code: 1, userInfo: [NSLocalizedDescriptionKey: "窓 \(w) が SCShareableContent に無い（\(content.windows.count) 個）"])
            }
            let filter = SCContentFilter(desktopIndependentWindow: win)
            let cfg = SCStreamConfiguration()
            let scale = CGFloat(filter.pointPixelScale)
            cfg.width = Int(win.frame.width * scale)
            cfg.height = Int(win.frame.height * scale)
            cfg.showsCursor = false
            let img = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: cfg)
            let url = URL(fileURLWithPath: out)
            guard let dst = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
                throw NSError(domain: "wincap", code: 1, userInfo: [NSLocalizedDescriptionKey: "\(out) に書けない"])
            }
            CGImageDestinationAddImage(dst, img, nil)
            guard CGImageDestinationFinalize(dst) else {
                throw NSError(domain: "wincap", code: 1, userInfo: [NSLocalizedDescriptionKey: "PNG を書けない"])
            }
            print("ok \(img.width)x\(img.height) \(out)")
        } catch {
            FileHandle.standardError.write("wincap: \(error.localizedDescription)\n".data(using: .utf8)!)
            exit(1)
        }
    }
}
