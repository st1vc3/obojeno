import AppKit
import SwiftUI

@main
struct ObojenoApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate

    var body: some Scene {
        // Everything lives in the menu bar item; see StatusPanel.
        Settings { EmptyView() }
    }
}

/// Facts about this build of the app.
enum AppInfo {
    static let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var model: AppModel?
    private var status: StatusPanel?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let model = AppModel()
        self.model = model
        status = StatusPanel(model: model)
    }
}

/// A dot in the current wallpaper's color; a palette symbol while there is
/// no theme yet.
enum MenuBarIcon {
    static func image(source: String?) -> NSImage {
        guard let source else {
            let symbol = NSImage(systemSymbolName: "paintpalette", accessibilityDescription: "obojeno")!
            symbol.isTemplate = true
            return symbol
        }
        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: false) { rect in
            let dot = NSBezierPath(ovalIn: rect.insetBy(dx: 3.5, dy: 3.5))
            NSColor(Color(hex: source)).setFill()
            dot.fill()
            // A hairline keeps light and dark colors visible on any bar.
            NSColor.labelColor.withAlphaComponent(0.35).setStroke()
            dot.lineWidth = 1
            dot.stroke()
            return true
        }
        image.isTemplate = false
        return image
    }
}
