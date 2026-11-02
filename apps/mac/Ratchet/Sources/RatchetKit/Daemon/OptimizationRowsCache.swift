import Foundation
import RatchetAPI

/// One row of the table: the list entry, the app's model of it, and the detail it was enriched from.
struct OptimizationRow: Sendable {
    var vm: VM.OptimizationVm
    var optimization: Optimization
    var detail: VM.OptimizationDetail?
}

/// What the daemon client remembers between calls about optimizations.
///
/// * The rows load: a refresh asks for the table and the benchmarks within moments of each other, and they share one load
///   (started within the last `lifetime` seconds).
/// * The detail of a finished optimization: it cannot change any more, so the table does not fetch it again while the list
///   entry is unchanged (the N+1 of every refresh). Anything that can change one (stop, pull request, start) calls `forget()`.
/// * Which optimization an agent belongs to (the daemon has no "agent by id" route).
actor OptimizationRowsCache {
    private var current: (task: Task<[OptimizationRow], any Error>, at: Date)?
    private var finished: [String: (vm: VM.OptimizationVm, detail: VM.OptimizationDetail)] = [:]
    private var agentOwners: [String: String] = [:]
    private let lifetime: TimeInterval

    init(lifetime: TimeInterval = 2) {
        self.lifetime = lifetime
    }

    /// The load started within the last `lifetime` seconds, or a new one made by `load`. A failed load is forgotten.
    func rows(load: @escaping @Sendable () async throws -> [OptimizationRow]) async throws -> [OptimizationRow] {
        let task: Task<[OptimizationRow], any Error>
        if let current, Date().timeIntervalSince(current.at) < lifetime {
            task = current.task
        } else {
            task = Task { try await load() }
            current = (task, Date())
        }
        do {
            return try await task.value
        } catch {
            current = nil
            throw error
        }
    }

    /// Drops the shared load and the settled details: something that changes an optimization just happened.
    func forget() {
        current = nil
        finished = [:]
    }

    // MARK: Finished optimizations

    static func isFinished(_ vm: VM.OptimizationVm) -> Bool {
        vm.status == .completed || vm.status == .stopped || vm.status == .failed
    }

    /// The detail kept for this list entry, if the entry has not changed since it was kept.
    func settled(_ vm: VM.OptimizationVm) -> VM.OptimizationDetail? {
        guard let kept = finished[vm.id], kept.vm == vm else { return nil }
        return kept.detail
    }

    func keep(_ vm: VM.OptimizationVm, detail: VM.OptimizationDetail) {
        guard Self.isFinished(vm) else { return }
        finished[vm.id] = (vm, detail)
    }

    // MARK: Agents

    func remember(agents: [String], of optimizationID: String) {
        for agent in agents { agentOwners[agent] = optimizationID }
    }

    func owner(ofAgent id: String) -> String? { agentOwners[id] }
}

/// Runs `body` over `items` with at most `limit` in flight, and returns the results in the order of `items`.
enum Bounded {
    static func map<T: Sendable, R: Sendable>(
        _ items: [T], limit: Int, _ body: @escaping @Sendable (T) async -> R
    ) async -> [R] {
        guard !items.isEmpty else { return [] }
        return await withTaskGroup(of: (Int, R).self) { group in
            var results: [Int: R] = [:]
            var next = 0
            while next < min(max(limit, 1), items.count) {
                let index = next
                let item = items[index]
                group.addTask { (index, await body(item)) }
                next += 1
            }
            while let (index, value) = await group.next() {
                results[index] = value
                if next < items.count {
                    let started = next
                    let item = items[started]
                    group.addTask { (started, await body(item)) }
                    next += 1
                }
            }
            return items.indices.compactMap { results[$0] }
        }
    }
}
