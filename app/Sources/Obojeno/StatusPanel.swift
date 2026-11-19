// The menu bar item and its panel. A borderless panel instead of a
// MenuBarExtra window, so the whole panel is real Liquid Glass over what is
// behind it rather than the system's window material.

import AppKit
import Observation
import SwiftUI

@MainActor
final class StatusPanel: NSObject {
    private let model: AppModel
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let panel = GlassPanel()
    private let glass = NSGlassEffectView()
    private let host: NSHostingController<AnyView>
    private var sizeObservation: NSKeyValueObservation?
    private var closedAt = Date.distantPast
    private var welcomed = false

    static let cornerRadius: CGFloat = 26

    init(model: AppModel) {
        self.model = model
        let settings = ProcessInfo.processInfo.environment["OBOJENO_OPEN"] == "settings"
        host = NSHostingController(rootView: AnyView(PopoverView(startWithSettings: settings).environment(model)))
        super.init()
        host.sizingOptions = [.preferredContentSize]
        // Clear glass, like WallBe: the wallpaper shows through, refracted.
        glass.style = .clear
        glass.cornerRadius = Self.cornerRadius
        glass.contentView = host.view
        panel.contentView = glass
        panel.onClose = { [weak self] in
            self?.closedAt = Date()
            // Back to the app that was in front, unless obojeno has other
            // windows up (its Dock icon, an open panel).
            if NSApp.windows.allSatisfy({ !$0.isVisible || $0.level == .statusBar }) { NSApp.hide(nil) }
        }
        sizeObservation = host.observe(\.preferredContentSize) { [weak self] _, _ in
            MainActor.assumeIsolated { self?.fitToContent() }
        }
        if let button = item.button {
            button.target = self
            button.action = #selector(toggle)
            button.setAccessibilityLabel("obojeno")
        }
        updateIcon()
        welcomeOnFirstRun()
        // A developer aid: open right away (OBOJENO_OPEN=1, or =settings on
        // the settings), to look at it without a click.
        if ProcessInfo.processInfo.environment["OBOJENO_OPEN"] != nil {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in self?.open() }
        }
    }

    /// Follows the wallpaper's color.
    private func updateIcon() {
        withObservationTracking {
            item.button?.image = MenuBarIcon.image(source: model.state?.palette.source)
        } onChange: { [weak self] in
            Task { @MainActor in self?.updateIcon() }
        }
    }

    /// Opens the panel by itself once on the first run, so the welcome is
    /// seen without finding the menu bar item first.
    private func welcomeOnFirstRun() {
        let needsWelcome = withObservationTracking {
            model.needsWelcome
        } onChange: { [weak self] in
            Task { @MainActor in self?.welcomeOnFirstRun() }
        }
        guard needsWelcome, !welcomed else { return }
        welcomed = true
        open()
    }

    @objc private func toggle() {
        if panel.isVisible {
            panel.close()
        } else if Date().timeIntervalSince(closedAt) > 0.25 {
            // A click on the item while open first closes the panel (it
            // loses key); don't reopen it right away.
            open()
        }
    }

    private func open() {
        fitToContent()
        panel.alphaValue = 0
        // Active, so controls draw in color rather than the gray of an app
        // in the background.
        NSApp.activate()
        panel.makeKeyAndOrderFront(nil)
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            panel.animator().alphaValue = 1
        }
    }

    /// Sizes the panel to its content, hanging below the menu bar item and
    /// kept on screen.
    private func fitToContent() {
        let size = host.preferredContentSize
        guard size.width > 0, size.height > 0,
            let button = item.button, let window = button.window, let screen = window.screen
        else { return }
        let anchor = window.convertToScreen(button.convert(button.bounds, to: nil))
        let area = screen.visibleFrame
        let x = min(max(anchor.midX - size.width / 2, area.minX + 8), area.maxX - size.width - 8)
        let top = anchor.minY - 6
        panel.setFrame(NSRect(x: x, y: top - size.height, width: size.width, height: size.height), display: true)
    }
}

/// Borderless and clear, so only the glass shows; it takes keys (shortcut
/// recording, Escape) and closes when it loses them, like a menu.
private final class GlassPanel: NSPanel {
    var onClose: (() -> Void)?

    init() {
        super.init(
            contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        level = .popUpMenu
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isReleasedWhenClosed = false
        hidesOnDeactivate = false
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
    }

    override var canBecomeKey: Bool { true }

    override func resignKey() {
        super.resignKey()
        // Not while a sheet or menu of our own is up.
        if attachedSheet == nil { close() }
    }

    override func cancelOperation(_ sender: Any?) { close() }

    override func close() {
        guard isVisible else { return }
        super.close()
        onClose?()
    }
}
