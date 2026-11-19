// A developer aid: with OBOJENO_SNAPSHOT=/path/out.png the app renders the
// popover offscreen with live daemon data, writes it as a PNG and quits.
// OBOJENO_SNAPSHOT_SETTINGS=1 shows the settings instead of the gallery.

import AppKit
import SwiftUI

@MainActor
enum Snapshot {
    static var requested: String? { ProcessInfo.processInfo.environment["OBOJENO_SNAPSHOT"] }
    static var showSettings: Bool { ProcessInfo.processInfo.environment["OBOJENO_SNAPSHOT_SETTINGS"] == "1" }

    static func run(model: AppModel, to path: String) {
        let root = PopoverView(startWithSettings: showSettings)
            .environment(model)
            .background(.windowBackground)
        let host = NSHostingView(rootView: root)
        let window = NSWindow(
            contentRect: NSRect(x: -10_000, y: 0, width: 380, height: 700),
            styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = host
        window.orderFrontRegardless()

        // Wait for the daemon's state and gallery, then for thumbnails.
        Task { @MainActor in
            for _ in 0..<50 where model.state == nil || model.gallery == nil {
                try? await Task.sleep(for: .milliseconds(100))
            }
            try? await Task.sleep(for: .seconds(1.5))
            let size = host.fittingSize
            window.setContentSize(size)
            host.layoutSubtreeIfNeeded()
            try? await Task.sleep(for: .milliseconds(300))
            guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else {
                FileHandle.standardError.write(Data("snapshot: no bitmap\n".utf8))
                exit(1)
            }
            host.cacheDisplay(in: host.bounds, to: rep)
            guard let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
            do {
                try png.write(to: URL(fileURLWithPath: path))
                print("wrote \(path) (\(Int(size.width))x\(Int(size.height)) pt)")
                exit(0)
            } catch {
                FileHandle.standardError.write(Data("snapshot: \(error)\n".utf8))
                exit(1)
            }
        }
    }
}
