import Foundation
import HTTPTypes
import OpenAPIRuntime
import RatchetAPI

/// `RatchetClient` on the real daemon: the generated OpenAPI client over the unix socket, the daemon's view models
/// mapped to the app's models, the event stream turned into `RatchetEvent`s.
///
/// Every group of the API is real: the overview, connectors and environment drafts included. There is no sample-data
/// fallback; a daemon that cannot answer is an error the app shows (and retries).
///
/// The pieces: `DaemonTransport` (the wire and its errors), `OptimizationRowsCache` (what is remembered between calls),
/// `DaemonEventFollower` (the event stream), and one file per domain (`+Optimizations`, `+Insights`, `+Connectors`,
/// `+Setup`, `+Sandbox`, `+Events`). This file holds only what several domains share.
public final class DaemonRatchetClient: RatchetClient {
    let wire: DaemonTransport
    let api: Client
    let cache = OptimizationRowsCache()
    /// How long events wait to be merged before the daemon is asked what changed.
    let coalescing: Duration

    /// Details fetched side by side when the table loads.
    static let detailConcurrency = 5

    init(wire: DaemonTransport, coalescing: Duration) {
        self.wire = wire
        self.api = wire.api
        self.coalescing = coalescing
    }

    public convenience init(transport: any ClientTransport, coalescing: Duration = .milliseconds(150)) {
        self.init(wire: DaemonTransport(transport: transport), coalescing: coalescing)
    }

    public convenience init(location: DaemonLocation) {
        self.init(wire: DaemonTransport(location: location), coalescing: .milliseconds(150))
    }

    // MARK: Fetching, shared by the domains

    func optimizationDetail(_ id: String) async throws -> VM.OptimizationDetail {
        let detail = try await api.getOptimization(path: .init(id: id)).ok.body.json
        await cache.remember(agents: detail.agents.map(\.id), of: id)
        return detail
    }

    func optimizationSnapshot() async throws -> [VM.OptimizationVm] {
        try await api.listOptimizations().ok.body.json.optimizations
    }

    /// Every optimization with its detail. A refresh asks for the table and the benchmarks within moments of each other;
    /// they share one load.
    func optimizationRows() async throws -> [OptimizationRow] {
        try await cache.rows { try await self.loadOptimizationRows() }
    }

    /// The list, then the details, `detailConcurrency` at a time. The table needs the clock, the pull request and the noise,
    /// which only the detail has; one that fails to load shows with what the list knows, and the detail of a finished
    /// optimization is not fetched again while its list entry is unchanged.
    private func loadOptimizationRows() async throws -> [OptimizationRow] {
        let list = try await optimizationSnapshot()
        let details = await Bounded.map(list, limit: Self.detailConcurrency) { vm -> VM.OptimizationDetail? in
            if let kept = await self.cache.settled(vm) { return kept }
            guard let detail = try? await self.optimizationDetail(vm.id) else { return nil }
            await self.cache.keep(vm, detail: detail)
            return detail
        }
        return zip(list, details).map { vm, detail in
            OptimizationRow(vm: vm, optimization: DaemonMapping.optimization(vm, detail: detail), detail: detail)
        }
    }

    func fetchOptimization(_ id: String) async throws -> Optimization {
        let detail = try await optimizationDetail(id)
        return DaemonMapping.optimization(detail.summary, detail: detail)
    }

    func environments() async throws -> VM.EnvironmentsVm {
        try await api.listEnvironments().ok.body.json
    }
}
