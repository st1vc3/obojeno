import AppKit
import SwiftUI

@main
struct ObojenoApp: App {
    @ViewState private var model = AppModel()

    init() {
        if let path = Snapshot.requested {
            let model = model
            DispatchQueue.main.async { Snapshot.run(model: model, to: path) }
        }
    }

    var body: some Scene {
        MenuBarExtra {
            PopoverView()
                .environment(model)
        } label: {
            Image(nsImage: MenuBarIcon.image(source: model.state?.palette.source))
                .accessibilityLabel("obojeno")
        }
        .menuBarExtraStyle(.window)
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
