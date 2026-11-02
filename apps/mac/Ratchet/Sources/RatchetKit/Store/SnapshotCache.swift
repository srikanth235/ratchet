import Foundation
import OSLog

/// What the app shows right after launch, before the daemon has answered.
struct CachedState: Codable, Sendable {
    var overview: Overview?
    var optimizations: [Optimization]
    var connectors: [Connector]
    var templates: [Template]
    var benchmarks: [BenchmarkRun]
    var repos: [Repo]
    var sandbox: SandboxStatus
    var receipt: TeardownReceipt?
}

/// The last snapshot on disk. Reads, encodes and writes happen on this actor, never on the main actor; a write waits `delay`
/// after the last `save`, so a burst of refreshes is one write; a failure is logged, not swallowed.
actor SnapshotCache {
    private var url: URL?
    private var pending: CachedState?
    private var writer: Task<Void, Never>?
    private let delay: Duration
    private let log = Logger(subsystem: "com.ratchet.app", category: "snapshot-cache")

    init(url: URL?, delay: Duration = .milliseconds(600)) {
        self.url = url
        self.delay = delay
    }

    /// Points the cache at another file (demo mode keeps its own) and forgets a write that has not happened yet.
    func use(_ url: URL?) {
        writer?.cancel()
        writer = nil
        pending = nil
        self.url = url
    }

    func load() -> CachedState? {
        guard let url else { return nil }
        guard let data = try? Data(contentsOf: url) else { return nil }
        do {
            return try JSONDecoder().decode(CachedState.self, from: data)
        } catch {
            log.error("could not read \(url.lastPathComponent, privacy: .public): \(error.localizedDescription, privacy: .public)")
            return nil
        }
    }

    /// Writes `state` after the pause, unless a newer one arrives first.
    func save(_ state: CachedState) {
        guard url != nil else { return }
        pending = state
        guard writer == nil else { return }
        writer = Task { [delay] in
            try? await Task.sleep(for: delay)
            await self.writePending()
        }
    }

    /// Writes what is waiting now (tests, and quitting).
    func flush() {
        writer?.cancel()
        writer = nil
        writePending()
    }

    private func writePending() {
        writer = nil
        guard let url, let state = pending else { return }
        pending = nil
        do {
            let data = try JSONEncoder().encode(state)
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try data.write(to: url, options: .atomic)
        } catch {
            log.error("could not write \(url.lastPathComponent, privacy: .public): \(error.localizedDescription, privacy: .public)")
        }
    }
}
