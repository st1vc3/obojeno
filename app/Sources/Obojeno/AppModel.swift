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
    /// The apps obojeno can theme, for the welcome and Apps settings.
    private(set) var targets: [TargetInfo] = []
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
            refreshTargets()
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
        case .targets(let targets):
            self.targets = targets
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

    /// Switches and sliders in the wallpaper's own color, light enough to
    /// read as "on" against the dark glass.
    var accent: Color { state?.palette.source(tone: 75) ?? .white }

    /// The first run: no config yet.
    var needsWelcome: Bool { state?.configured == false }

    func refreshTargets() {
        run { client in
            if case .targets(let targets) = try await client.send(.targets) {
                self.targets = targets
            }
        }
    }

    /// Turns an app's theming on or off, with the options found for it and
    /// any chosen now.
    func setTarget(_ target: TargetInfo, enabled: Bool, options: [String: String] = [:]) {
        let all = target.options.merging(options) { _, chosen in chosen }
        let change = ConfigChange(targets: [target.name: TargetChange(enabled: enabled, options: all)])
        run { client in
            _ = try await client.send(.configure(change))
            if case .targets(let targets) = try await client.send(.targets) {
                self.targets = targets
            }
        }
    }

    /// Ends the first run: the chosen apps on, the other ones found off.
    /// Writing the config is what marks the first run as done.
    func finishWelcome(enabling chosen: Set<String>, options: [String: [String: String]]) {
        var changes: [String: TargetChange] = [:]
        for target in targets where target.installed {
            let all = target.options.merging(options[target.name] ?? [:]) { _, chosen in chosen }
            guard target.missing.map({ all[$0] != nil }) ?? true else { continue }
            changes[target.name] = TargetChange(enabled: chosen.contains(target.name), options: all)
        }
        var change = ConfigChange(targets: changes)
        // Something to pick from right away: the wallpapers macOS comes with.
        if state?.wallpaperDirs.isEmpty ?? true {
            change.wallpaperDirs = ["/System/Library/Desktop Pictures"]
        }
        run { client in
            _ = try await client.send(.configure(change))
            if case .targets(let targets) = try await client.send(.targets) {
                self.targets = targets
            }
        }
    }

    /// Asks for a folder, such as an Obsidian vault, as `~/...` when it is
    /// in the home folder.
    func chooseFolder(for target: TargetInfo, option: String, then done: @escaping @MainActor (String) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Choose the \(option) folder for \(target.title)"
        panel.prompt = "Choose"
        present(panel) { urls in done(self.abbreviate(urls[0].path)) }
    }

    func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
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
        present(panel) { urls in
            let paths = urls.map(\.path)
            self.run { client in
                if case .gallery(let gallery) = try await client.send(.importImages(paths)) {
                    self.gallery = gallery
                }
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
        present(panel) { urls in
            let dirs = (self.state?.wallpaperDirs ?? []) + urls.map { self.abbreviate($0.path) }
            self.configure(ConfigChange(wallpaperDirs: dirs))
        }
    }

    /// Shows an open panel as a sheet on the menu bar panel, which then
    /// stays open behind it; as a window of its own when that is closed.
    private func present(_ panel: NSOpenPanel, then done: @escaping @MainActor ([URL]) -> Void) {
        NSApp.activate()
        if let window = NSApp.keyWindow {
            panel.beginSheetModal(for: window) { response in
                MainActor.assumeIsolated {
                    if response == .OK, !panel.urls.isEmpty { done(panel.urls) }
                }
            }
        } else if panel.runModal() == .OK, !panel.urls.isEmpty {
            done(panel.urls)
        }
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
