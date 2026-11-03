// Docking の試験に使う、何も持たない普通の窓の app（kangae-portal --measure の相手。人の app の窓を動かさずに追従を測る）。
// 文書を持たず、保存もしない。格子を描くだけ。build と .app への包み方は docs/internal/p-portal-owner-steps.md。
//   swiftc -O -swift-version 5 tests/dock-target.swift -o <dir>/KangaeDockTarget.app/Contents/MacOS/KangaeDockTarget
import AppKit

final class Grid: NSView {
    override func draw(_ dirty: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        dirty.fill()
        NSColor.gridColor.setStroke()
        let p = NSBezierPath()
        var x: CGFloat = 0
        while x <= bounds.width {
            p.move(to: NSPoint(x: x, y: 0))
            p.line(to: NSPoint(x: x, y: bounds.height))
            x += 40
        }
        var y: CGFloat = 0
        while y <= bounds.height {
            p.move(to: NSPoint(x: 0, y: y))
            p.line(to: NSPoint(x: bounds.width, y: y))
            y += 40
        }
        p.stroke()
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(
    contentRect: NSRect(x: 240, y: 240, width: 640, height: 420),
    styleMask: [.titled, .closable, .resizable, .miniaturizable],
    backing: .buffered,
    defer: false
)
window.title = "Kangae dock target"
window.contentView = Grid()
window.makeKeyAndOrderFront(nil)
app.run()
