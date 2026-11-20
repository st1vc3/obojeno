// Keeps the app current. Downloaded installs update themselves with Sparkle:
// it checks daily (when enabled) or when asked, verifies the update's EdDSA
// signature, replaces the app and relaunches it. Homebrew and Nix installs
// update the way they were installed, so for them the app only looks for a
// newer release on GitHub and says how to get it.

import AppKit
import Observation
import Sparkle

struct Release: Equatable {
    let version: String
    let page: URL

    init(version: String, page: URL) {
        self.version = version
        self.page = page
    }

    /// A version published on GitHub.
    init(version: String) {
        self.init(version: version, page: Updater.releasePage(version))
    }
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
    nonisolated static let repository = "st1vc3/obojeno"
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

    /// Whether updates install from within the app.
    var installsUpdates: Bool { sparkle != nil }

    /// Sparkle, for downloaded installs whose bundle names an update feed.
    @ObservationIgnored private var sparkle: SparkleUpdates?
    @ObservationIgnored private var loop: Task<Void, Never>?

    init() {
        if InstallMethod.current == .download, SparkleUpdates.isConfigured {
            sparkle = SparkleUpdates(updater: self)
        }
        schedule()
    }

    func dismiss() {
        available = nil
        message = nil
    }

    /// Brings up Sparkle's window for the update found, to install it.
    func install() {
        available = nil
        sparkle?.showUpdate()
    }

    /// Sparkle found an update in the background: shown as the banner, as
    /// a window would open behind whatever the user is doing.
    fileprivate func found(_ version: String) {
        available = Release(version: version)
        message = nil
    }

    fileprivate func sessionEnded() {
        available = nil
    }

    private func schedule() {
        loop?.cancel()
        if let sparkle {
            sparkle.automatic = automatic
            return
        }
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
    /// user asked for always say how they went (Sparkle's own window, for
    /// installs it updates).
    func check(userInitiated: Bool) async {
        if let sparkle {
            sparkle.showUpdate()
            return
        }
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

    nonisolated static func releasePage(_ version: String) -> URL {
        URL(string: "https://github.com/\(repository)/releases/tag/v\(version)")!
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

/// Sparkle with "gentle reminders", as menu bar apps should use it: an
/// update found in the background goes to the banner, and Sparkle's window
/// only opens when the user asks for it.
@MainActor
private final class SparkleUpdates: NSObject, SPUStandardUserDriverDelegate {
    /// build-app.sh writes the feed and the public key into Info.plist.
    static var isConfigured: Bool {
        Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil
            && Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") != nil
    }

    private weak var owner: Updater?
    private var controller: SPUStandardUpdaterController?

    init(updater: Updater) {
        owner = updater
        super.init()
        controller = SPUStandardUpdaterController(
            startingUpdater: true, updaterDelegate: nil, userDriverDelegate: self)
    }

    var automatic: Bool {
        get { controller?.updater.automaticallyChecksForUpdates ?? false }
        set { controller?.updater.automaticallyChecksForUpdates = newValue }
    }

    /// Checks now and shows the result in Sparkle's window, in front.
    func showUpdate() {
        NSApp.activate()
        controller?.checkForUpdates(nil)
    }

    nonisolated var supportsGentleScheduledUpdateReminders: Bool { true }

    nonisolated func standardUserDriverShouldHandleShowingScheduledUpdate(
        _ update: SUAppcastItem, andInImmediateFocus immediateFocus: Bool
    ) -> Bool {
        // Sparkle's window only when it would open in front anyway.
        immediateFocus
    }

    nonisolated func standardUserDriverWillHandleShowingUpdate(
        _ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem, state: SPUUserUpdateState
    ) {
        guard !handleShowingUpdate else { return }
        let version = update.displayVersionString
        MainActor.assumeIsolated { owner?.found(version) }
    }

    nonisolated func standardUserDriverWillFinishUpdateSession() {
        MainActor.assumeIsolated { owner?.sessionEnded() }
    }
}
