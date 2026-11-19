import AppKit
import Observation
import ServiceManagement
import SwiftUI

/// How large gallery tiles are; the toolbar switches between them.
enum Density: String, CaseIterable {
    case small, medium, large

    var columns: Int {
        switch self {
        case .small: 4
        case .medium: 3
        case .large: 2
        }
    }

    var symbol: String {
        switch self {
        case .small: "square.grid.3x3.fill"
        case .medium: "square.grid.2x2.fill"
        case .large: "rectangle.grid.1x2.fill"
        }
    }
}

@MainActor @Observable
final class AppModel {
    private(set) var state: DaemonState?
    private(set) var gallery: Gallery?
    private(set) var connected = false
    /// The last failed action, shown until the next one succeeds.
    var error: String?
    /// A wallpaper the user picked, shown as current until the daemon confirms.
    private(set) var choosing: String?

    var density: Density = Density(rawValue: UserDefaults.standard.string(forKey: "density") ?? "") ?? .medium {
        didSet { UserDefaults.standard.set(density.rawValue, forKey: "density") }
    }

    var showInDock: Bool = UserDefaults.standard.bool(forKey: "showInDock") {
        didSet {
            UserDefaults.standard.set(showInDock, forKey: "showInDock")
            applyDockPolicy()
        }
    }

    /// Mirrors the login item; read back after every change since macOS can
    /// refuse or need approval in System Settings.
    private(set) var openAtLogin = SMAppService.mainApp.status == .enabled

    let shortcuts = Shortcuts()
    let updater = Updater()
    private let hint = ShortcutHint()

    private let client = DaemonClient()
    private var subscription: Task<Void, Never>?
    let supervisor = DaemonSupervisor()

    init() {
        applyDockPolicy()
        shortcuts.onTrigger = { [weak self] direction in
            guard let self else { return }
            self.step(direction)
            if let combo = self.shortcuts.combos[direction] { self.hint.show(direction, keys: combo.keys) }
        }
        shortcuts.registerAll()
        subscription = Task { [weak self, client] in
            // One ordered stream, so a late "disconnected" can never
            // overwrite a newer "connected".
            for await update in client.subscribe() {
                switch update {
                case .connected(let pid): self?.connectionChanged(true, pid: pid)
                case .disconnected: self?.connectionChanged(false, pid: nil)
                case .reply(let reply): self?.receive(reply)
                }
            }
        }
    }

    var mode: Mode { state?.mode ?? .dark }

    func color(_ role: String) -> Color {
        state.map { $0.palette.color(role, $0.mode) } ?? .secondary
    }

    var currentItem: GalleryItem? {
        guard let wallpaper = state?.wallpaper else { return nil }
        return gallery?.allItems.first { $0.path == wallpaper }
    }

    private func connectionChanged(_ connected: Bool, pid: pid_t?) {
        self.connected = connected
        if connected {
            supervisor.connected(to: pid)
            refreshGallery()
        } else if Snapshot.requested == nil {
            // Installed without a login agent (Homebrew, a download), or the
            // daemon crashed: the app brings its own.
            supervisor.disconnected()
        }
    }

    private func receive(_ reply: Reply) {
        switch reply {
        case .state(let state):
            supervisor.check(version: state.version)
            withAnimation(.easeInOut(duration: 0.4)) { self.state = state }
            // Current markers and swatches follow the theme.
            refreshGallery()
        case .gallery(let gallery):
            self.gallery = gallery
        case .report, .error:
            break
        }
    }

    func refreshGallery() {
        run { client in
            if case .gallery(let gallery) = try await client.send(.gallery) {
                self.gallery = gallery
            }
        }
    }

    func setWallpaper(_ path: String) {
        choosing = path
        // The circle transition grows from where the click happened.
        let origin = NSEvent.mouseLocation
        run { client in
            defer { self.choosing = nil }
            _ = try await client.send(.setWallpaper(path: path, origin: origin))
        }
    }

    func step(_ direction: Direction) {
        run { client in _ = try await client.send(.step(direction)) }
    }

    func toggleMode() {
        let next: Mode = mode == .dark ? .light : .dark
        run { client in _ = try await client.send(.setMode(next)) }
    }

    func configure(_ change: ConfigChange) {
        run { client in _ = try await client.send(.configure(change)) }
    }

    /// Copies images into the library.
    func importImages() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.allowsMultipleSelection = true
        panel.message = "Choose images to add to your obojeno library"
        panel.prompt = "Add to Library"
        NSApp.activate()
        guard panel.runModal() == .OK, !panel.urls.isEmpty else { return }
        let paths = panel.urls.map(\.path)
        run { client in
            if case .gallery(let gallery) = try await client.send(.importImages(paths)) {
                self.gallery = gallery
            }
        }
    }

    func removeFromLibrary(_ item: GalleryItem) {
        run { client in
            if case .gallery(let gallery) = try await client.send(.remove(item.path)) {
                self.gallery = gallery
            }
        }
    }

    func showInFinder(_ item: GalleryItem) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: item.path)])
    }

    func addFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Choose a folder of wallpapers to show in the gallery"
        panel.prompt = "Add Folder"
        NSApp.activate()
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let dirs = (state?.wallpaperDirs ?? []) + [abbreviate(url.path)]
        configure(ConfigChange(wallpaperDirs: dirs))
    }

    func removeFolder(_ dir: String) {
        configure(ConfigChange(wallpaperDirs: (state?.wallpaperDirs ?? []).filter { $0 != dir }))
    }

    func setOpenAtLogin(_ enabled: Bool) {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            error = nil
        } catch {
            self.error = "Open at login: \(error.localizedDescription)"
        }
        openAtLogin = SMAppService.mainApp.status == .enabled
    }

    private func applyDockPolicy() {
        NSApp?.setActivationPolicy(showInDock ? .regular : .accessory)
    }

    /// Starts the daemon now, from the "not running" view.
    func startDaemon() {
        supervisor.start()
        error = supervisor.error
    }

    func showLog() {
        NSWorkspace.shared.open(DaemonSupervisor.logFile)
    }

    private func run(_ action: @escaping @MainActor (DaemonClient) async throws -> Void) {
        Task {
            do {
                try await action(client)
                error = nil
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    private func abbreviate(_ path: String) -> String {
        let home = NSHomeDirectory()
        return path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
    }

}
