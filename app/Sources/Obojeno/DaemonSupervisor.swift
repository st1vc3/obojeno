// Keeps the app's own daemon running and current: started when none runs,
// restarted after a crash (waiting longer each time it keeps failing), and
// replaced when it is older than the app, as after an update. A daemon from
// somewhere else (a login agent, Nix) is left alone.

import AppKit
import Darwin

@MainActor
final class DaemonSupervisor {
    static let logFile = URL(fileURLWithPath: NSHomeDirectory() + "/Library/Logs/obojeno.log")
    /// The log is moved aside to `obojeno.log.1` beyond this size.
    private static let maxLogSize = 5 * 1024 * 1024

    /// The last start failed, for the "not running" view.
    private(set) var error: String?

    private var failures = 0
    private var connectedSince: Date?
    private var pending: Task<Void, Never>?
    /// The daemon connected to, whose states arrive now.
    private var daemonPID: pid_t?
    /// Daemons already replaced, so each is only stopped once.
    private var replaced: Set<pid_t> = []

    // MARK: Events from the connection

    func connected(to pid: pid_t?) {
        pending?.cancel()
        pending = nil
        connectedSince = Date()
        daemonPID = pid
        error = nil
    }

    func disconnected() {
        // A daemon that ran for a while before stopping is not failing.
        if let since = connectedSince, Date().timeIntervalSince(since) > 30 { failures = 0 }
        connectedSince = nil
        daemonPID = nil
        guard pending == nil else { return }
        let delay = failures == 0 ? 0 : min(30, pow(2, Double(failures - 1)))
        failures += 1
        pending = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled else { return }
            self?.pending = nil
            self?.start()
            self?.expectConnection()
        }
    }

    /// A daemon that dies before the app connects to it never disconnects,
    /// so not connecting in time counts as a failure too.
    private func expectConnection() {
        pending = Task { [weak self] in
            try? await Task.sleep(for: .seconds(5))
            guard !Task.isCancelled, let self, self.connectedSince == nil else { return }
            self.pending = nil
            self.disconnected()
        }
    }

    /// Replaces the app's own daemon when the state it sent shows another
    /// version.
    func check(version: String?) {
        guard version != AppInfo.version, let pid = daemonPID, !replaced.contains(pid),
            let binary = Self.findBinary(), let path = Self.executablePath(of: pid),
            Self.sameFile(path, binary.path)
        else { return }
        // Its executable is ours, so it is the copy from before the update:
        // stop it, and `disconnected` starts the new one.
        replaced.insert(pid)
        kill(pid, SIGTERM)
    }

    // MARK: Starting

    /// Starts the daemon in the background, logging to `logFile`. Harmless
    /// when one already runs: the second one exits right away.
    func start() {
        guard let binary = Self.findBinary() else {
            error = "Cannot find the obojeno command. Install it or run `obojeno daemon`."
            return
        }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["--verbose", "daemon"]
        process.standardInput = FileHandle.nullDevice
        let log = Self.openLog()
        process.standardOutput = log ?? FileHandle.nullDevice
        process.standardError = log ?? FileHandle.nullDevice
        do {
            try process.run()
            error = nil
        } catch {
            self.error = "Starting the daemon failed: \(error.localizedDescription)"
        }
    }

    /// The log, opened for appending; an old one past `maxLogSize` is moved
    /// to `obojeno.log.1` first.
    private static func openLog() -> FileHandle? {
        let manager = FileManager.default
        let path = logFile.path
        try? manager.createDirectory(at: logFile.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let size = (try? manager.attributesOfItem(atPath: path))?[.size] as? Int, size > maxLogSize {
            let old = path + ".1"
            try? manager.removeItem(atPath: old)
            try? manager.moveItem(atPath: path, toPath: old)
        }
        if !manager.fileExists(atPath: path) { manager.createFile(atPath: path, contents: nil) }
        guard let handle = FileHandle(forWritingAtPath: path) else { return nil }
        handle.seekToEndOfFile()
        return handle
    }

    /// Next to the app first (bundled), then where package managers put it.
    static func findBinary() -> URL? {
        let user = NSUserName()
        var candidates = [Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/obojeno").path]
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

    private static func executablePath(of pid: pid_t) -> String? {
        var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
        let length = proc_pidpath(pid, &buffer, UInt32(buffer.count))
        guard length > 0 else { return nil }
        return String(decoding: buffer.prefix(Int(length)).map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }

    private static func sameFile(_ a: String, _ b: String) -> Bool {
        let resolve = { (path: String) in URL(fileURLWithPath: path).resolvingSymlinksInPath().path }
        return resolve(a) == resolve(b)
    }
}
