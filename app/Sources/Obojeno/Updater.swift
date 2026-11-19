// Looks for a newer release on GitHub: once a day when enabled, and when
// asked from the menu. Updating itself is left to whatever installed the
// app (Homebrew, Nix, a download).

import AppKit
import Observation

struct Release: Equatable {
    let version: String
    let page: URL
}

/// How this copy of the app was installed, which decides how to update it.
enum InstallMethod {
    case homebrew, nix, download

    static let current: InstallMethod = {
        let path = Bundle.main.bundleURL.resolvingSymlinksInPath().path
        if path.hasPrefix("/nix/store/") { return .nix }
        let casks = ["/opt/homebrew/Caskroom/obojeno", "/usr/local/Caskroom/obojeno"]
        if casks.contains(where: FileManager.default.fileExists(atPath:)) { return .homebrew }
        return .download
    }()
}

@MainActor @Observable
final class Updater {
    static let repository = "st1vc3/obojeno"
    static let brewCommand = "brew upgrade --cask obojeno"

    /// A newer release, until it is installed or dismissed.
    private(set) var available: Release?
    private(set) var checking = false
    /// The outcome of a check the user asked for, until dismissed.
    var message: String?

    var automatic: Bool = UserDefaults.standard.object(forKey: "checkForUpdates") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(automatic, forKey: "checkForUpdates")
            schedule()
        }
    }

    @ObservationIgnored private var loop: Task<Void, Never>?

    init() { schedule() }

    func dismiss() {
        available = nil
        message = nil
    }

    private func schedule() {
        loop?.cancel()
        guard automatic else { return }
        loop = Task { [weak self] in
            // Not while everything is still starting up at login.
            try? await Task.sleep(for: .seconds(15))
            while !Task.isCancelled {
                await self?.check(userInitiated: false)
                try? await Task.sleep(for: .seconds(24 * 60 * 60))
            }
        }
    }

    /// Automatic checks stay quiet unless there is an update; checks the
    /// user asked for always say how they went.
    func check(userInitiated: Bool) async {
        guard !checking else { return }
        checking = true
        defer { checking = false }
        do {
            let latest = try await Self.latestRelease()
            if let latest, Self.isNewer(latest.version, than: AppInfo.version) {
                available = latest
                message = nil
            } else if userInitiated {
                available = nil
                message =
                    latest == nil
                    ? "No releases are published yet."
                    : "obojeno \(AppInfo.version) is the newest version."
            }
        } catch {
            if userInitiated { message = "Checking for updates failed: \(error.localizedDescription)" }
        }
    }

    /// The latest release, or nil when there is none (or the repository is
    /// not public).
    private static func latestRelease() async throws -> Release? {
        var request = URLRequest(url: URL(string: "https://api.github.com/repos/\(repository)/releases/latest")!)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = 20
        let (data, response) = try await URLSession.shared.data(for: request)
        switch (response as? HTTPURLResponse)?.statusCode {
        case 200: break
        case 404: return nil
        default: throw URLError(.badServerResponse)
        }
        struct Body: Decodable {
            let tagName: String
            let htmlURL: URL
            enum CodingKeys: String, CodingKey {
                case tagName = "tag_name"
                case htmlURL = "html_url"
            }
        }
        let body = try JSONDecoder().decode(Body.self, from: data)
        let version = body.tagName.hasPrefix("v") ? String(body.tagName.dropFirst()) : body.tagName
        return Release(version: version, page: body.htmlURL)
    }

    /// Compares dotted version numbers: 0.10.0 is newer than 0.9.1, and
    /// missing parts count as 0.
    nonisolated static func isNewer(_ version: String, than current: String) -> Bool {
        let parts = { (v: String) in v.split(separator: ".").map { Int($0.prefix(while: \.isNumber)) ?? 0 } }
        let (a, b) = (parts(version), parts(current))
        for i in 0..<max(a.count, b.count) {
            let (x, y) = (i < a.count ? a[i] : 0, i < b.count ? b[i] : 0)
            if x != y { return x > y }
        }
        return false
    }
}
