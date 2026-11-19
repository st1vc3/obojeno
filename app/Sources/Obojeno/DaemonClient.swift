// Talking to `obojeno daemon` over its Unix socket: one JSON line per request
// and reply, plus a long-lived subscription for state and gallery updates.

import Foundation

enum DaemonError: LocalizedError {
    case notRunning
    case io(String)
    case daemon(String)

    var errorDescription: String? {
        switch self {
        case .notRunning: "The obojeno daemon is not running."
        case .io(let message): message
        case .daemon(let message): message
        }
    }
}

struct DaemonClient: Sendable {
    /// Same place the daemon uses: `$XDG_STATE_HOME/obojeno/obojeno.sock`.
    let socketPath: String = {
        let env = ProcessInfo.processInfo.environment
        let state = env["XDG_STATE_HOME"].flatMap { $0.hasPrefix("/") ? $0 : nil }
            ?? NSHomeDirectory() + "/.local/state"
        return state + "/obojeno/obojeno.sock"
    }()

    /// Sends one request and returns its reply.
    func send(_ request: Request) async throws -> Reply {
        let socketPath = socketPath
        return try await Task.detached {
            let connection = try Connection(path: socketPath)
            try connection.write(request)
            guard let reply = try connection.readReply() else {
                throw DaemonError.io("The daemon closed the connection.")
            }
            if case .error(let message) = reply { throw DaemonError.daemon(message) }
            return reply
        }.value
    }

    /// What a subscription delivers, in order.
    enum Update: Sendable {
        /// Connected to the daemon with this process id (nil if it cannot
        /// be told), or disconnected.
        case connected(pid_t?)
        case disconnected
        case reply(Reply)
    }

    /// Delivers connection changes and every state and gallery update, in
    /// order, until the task is cancelled; reconnects while the daemon is
    /// down.
    func subscribe() -> AsyncStream<Update> {
        let socketPath = socketPath
        return AsyncStream { continuation in
            let control = SubscriptionControl()
            let thread = Thread {
                // Nil until the first attempt, so a daemon that isn't
                // running at launch is reported too, not only lost ones.
                var connected: Bool?
                while !control.isCancelled {
                    if let connection = try? Connection(path: socketPath),
                        control.attach(connection),
                        (try? connection.write(Request.subscribe)) != nil
                    {
                        connected = true
                        continuation.yield(.connected(connection.peerPID()))
                        while let reply = try? connection.readReply() {
                            continuation.yield(.reply(reply))
                        }
                    }
                    if connected != false {
                        connected = false
                        continuation.yield(.disconnected)
                    }
                    if !control.isCancelled { Thread.sleep(forTimeInterval: 1) }
                }
                continuation.finish()
            }
            thread.name = "obojeno-subscription"
            continuation.onTermination = { _ in control.cancel() }
            thread.start()
        }
    }
}

/// Lets the subscription thread be stopped while it blocks in `read`.
private final class SubscriptionControl: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    private var connection: Connection?

    var isCancelled: Bool { lock.withLock { cancelled } }

    /// Remembers the live connection; `false` once cancelled.
    func attach(_ connection: Connection) -> Bool {
        lock.withLock {
            self.connection = connection
            return !cancelled
        }
    }

    func cancel() {
        lock.withLock {
            cancelled = true
            connection?.shutdown()
        }
    }
}

/// A blocking socket connection; used off the main thread only.
private final class Connection {
    private let fd: Int32
    private var buffer = Data()

    init(path: String) throws {
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw DaemonError.io("socket: \(String(cString: strerror(errno)))") }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8CString)
        guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else {
            close(fd)
            throw DaemonError.io("socket path too long: \(path)")
        }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            bytes.withUnsafeBytes { raw.copyMemory(from: $0) }
        }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            close(fd)
            throw DaemonError.notRunning
        }
    }

    deinit { close(fd) }

    /// The process on the other end.
    func peerPID() -> pid_t? {
        var pid: pid_t = 0
        var length = socklen_t(MemoryLayout<pid_t>.size)
        guard getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, &pid, &length) == 0 else { return nil }
        return pid
    }

    /// Wakes a blocked `read` on another thread; it then returns end of file.
    func shutdown() { Darwin.shutdown(fd, SHUT_RDWR) }

    func write(_ request: Request) throws {
        var data = try JSONEncoder().encode(request)
        data.append(UInt8(ascii: "\n"))
        try data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let n = Darwin.write(fd, raw.baseAddress! + offset, raw.count - offset)
                guard n > 0 else { throw DaemonError.io("write: \(String(cString: strerror(errno)))") }
                offset += n
            }
        }
    }

    /// The next reply, or `nil` when the daemon closed the connection.
    func readReply() throws -> Reply? {
        while true {
            if let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
                let line = buffer[buffer.startIndex..<newline]
                buffer.removeSubrange(buffer.startIndex...newline)
                return try JSONDecoder().decode(Reply.self, from: line)
            }
            var chunk = [UInt8](repeating: 0, count: 64 * 1024)
            let n = read(fd, &chunk, chunk.count)
            if n <= 0 { return nil }
            buffer.append(contentsOf: chunk[0..<n])
        }
    }
}
