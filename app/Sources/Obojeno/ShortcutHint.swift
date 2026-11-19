// A small glass hint at the bottom right of the screen when a shortcut is
// used: the keys and what they did, gone again after a moment.

import AppKit
import SwiftUI

@MainActor
final class ShortcutHint {
    private var panel: NSPanel?
    private var hide: Task<Void, Never>?

    func show(_ direction: Direction, keys: [String]) {
        let panel = self.panel ?? makePanel()
        self.panel = panel
        let host = NSHostingView(rootView: HintView(keys: keys, title: direction.title))
        panel.contentView = host
        let size = host.fittingSize
        // The screen being looked at: the one with the pointer.
        let mouse = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { $0.frame.contains(mouse) }) ?? NSScreen.main
        else { return }
        let area = screen.visibleFrame
        panel.setFrame(
            NSRect(x: area.maxX - size.width - 24, y: area.minY + 24, width: size.width, height: size.height),
            display: true)
        if !panel.isVisible {
            panel.alphaValue = 0
            panel.orderFrontRegardless()
        }
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.18
            panel.animator().alphaValue = 1
        }
        hide?.cancel()
        hide = Task { [weak panel] in
            try? await Task.sleep(for: .seconds(1.4))
            guard !Task.isCancelled, let panel else { return }
            await NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.35
                panel.animator().alphaValue = 0
            }
            if !Task.isCancelled { panel.orderOut(nil) }
        }
    }

    private func makePanel() -> NSPanel {
        let panel = NSPanel(
            contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.level = .statusBar
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        return panel
    }
}

private struct HintView: View {
    let keys: [String]
    let title: String

    var body: some View {
        GlassEffectContainer(spacing: 6) {
            HStack(spacing: 6) {
                ForEach(Array(keys.enumerated()), id: \.offset) { _, key in
                    Text(key)
                        .font(.system(size: 14, weight: .semibold))
                        .frame(minWidth: 30, minHeight: 30)
                        .padding(.horizontal, key.count > 1 ? 6 : 0)
                        .glassEffect(.regular, in: .rect(cornerRadius: 9))
                }
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .padding(.horizontal, 12)
                    .frame(minHeight: 30)
                    .glassEffect(.regular, in: .capsule)
            }
        }
        .foregroundStyle(.white)
        .environment(\.colorScheme, .dark)
        // Room for the glass's own shadow and rim.
        .padding(12)
    }
}

extension Direction {
    var title: String {
        switch self {
        case .next: "Next wallpaper"
        case .previous: "Previous wallpaper"
        case .random: "Random wallpaper"
        }
    }
}
