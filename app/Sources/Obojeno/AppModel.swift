import AppKit
import Observation
import SwiftUI

@MainActor @Observable
final class AppModel {
    private(set) var state: DaemonState?
    private(set) var gallery: Gallery?
    private(set) var connected = false
    /// The last failed action, shown until the next one succeeds.
    var error: String?
    /// A wallpaper the user picked, shown as current until the daemon confirms.
    private(set) var choosing: String?

    private let client = DaemonClient()
    private var subscription: Task<Void, Never>?

    init() {
        subscription = Task { [weak self, client] in
            let updates = client.subscribe { connected in
                Task { @MainActor in self?.connectionChanged(connected) }
            }
            for await reply in updates {
                self?.receive(reply)
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

    private func connectionChanged(_ connected: Bool) {
        self.connected = connected
        if connected { refreshGallery() }
    }

    private func receive(_ reply: Reply) {
        switch reply {
        case .state(let state):
            withAnimation(.easeInOut(duration: 0.3)) { self.state = state }
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
        run { client in
            defer { self.choosing = nil }
            _ = try await client.send(.setWallpaper(path: path))
        }
    }

    func setMode(_ mode: Mode) {
        guard mode != state?.mode else { return }
        run { client in _ = try await client.send(.setMode(mode)) }
    }

    func configure(_ change: ConfigChange) {
        run { client in _ = try await client.send(.configure(change)) }
    }

    func chooseImage() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.allowsMultipleSelection = false
        panel.message = "Choose a wallpaper"
        panel.prompt = "Set Wallpaper"
        NSApp.activate()
        if panel.runModal() == .OK, let url = panel.url {
            setWallpaper(url.path)
        }
    }

    func addFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.message = "Choose a folder of wallpapers"
        panel.prompt = "Add Folder"
        NSApp.activate()
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let dirs = (state?.wallpaperDirs ?? []) + [abbreviate(url.path)]
        configure(ConfigChange(wallpaperDirs: dirs))
    }

    func removeFolder(_ dir: String) {
        configure(ConfigChange(wallpaperDirs: (state?.wallpaperDirs ?? []).filter { $0 != dir }))
    }

    /// Starts the daemon; for when it is not set up to run at login.
    func startDaemon() {
        guard let binary = Self.findDaemonBinary() else {
            error = "Cannot find the obojeno command. Install it or run `obojeno daemon`."
            return
        }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["daemon"]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            self.error = "Starting the daemon failed: \(error.localizedDescription)"
        }
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

    /// Next to the app first (bundled), then where package managers put it.
    private static func findDaemonBinary() -> URL? {
        let user = NSUserName()
        var candidates = [
            Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/obojeno").path
        ]
        if let override = ProcessInfo.processInfo.environment["OBOJENO_BIN"] {
            candidates.insert(override, at: 0)
        }
        candidates += [
            "/opt/homebrew/bin/obojeno", "/usr/local/bin/obojeno",
            "/etc/profiles/per-user/\(user)/bin/obojeno", "/run/current-system/sw/bin/obojeno",
            NSHomeDirectory() + "/.nix-profile/bin/obojeno",
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
            .map { URL(fileURLWithPath: $0) }
    }
}
