import Foundation
import Observation

public enum OptimizationFilter: String, CaseIterable, Identifiable, Sendable {
    case all, running, done

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .all: "All"
        case .running: "Running"
        case .done: "Done"
        }
    }

    public func includes(_ optimization: Optimization) -> Bool {
        switch self {
        case .all: true
        case .running: optimization.isLive
        case .done: optimization.status.isFinished
        }
    }
}

/// The optimizations table, the details that were opened, and the actions on a run and its agents.
///
/// Details are kept for the most recently opened optimizations only (`detailLimit`): the oldest finished ones go first, and a
/// live optimization is never dropped. `AppStore` fills the list on a refresh and applies events through `EventReducer`.
@MainActor @Observable
public final class OptimizationsStore {
    /// How many details are kept.
    public static let detailLimit = 24

    public internal(set) var optimizations: [Optimization] = []
    public internal(set) var details: [String: OptimizationDetail] = [:]

    /// Opened longest ago first.
    @ObservationIgnored private var detailOrder: [String] = []
    private let client: any OptimizationsClient
    private let errors: StoreErrors
    /// How a change this store made reaches the rest (the overview's totals, notifications): `AppStore.apply`.
    @ObservationIgnored var onEvent: (@MainActor (RatchetEvent) -> Void)?
    /// An optimization the person just changed, shown without announcing it: `AppStore.show`.
    @ObservationIgnored var onChange: (@MainActor (Optimization) -> Void)?

    public init(client: any OptimizationsClient, errors: StoreErrors) {
        self.client = client
        self.errors = errors
    }

    func reset() {
        optimizations = []
        details = [:]
        detailOrder = []
    }

    // MARK: Derived

    public var runningOptimizations: [Optimization] { optimizations.filter { $0.status == .running } }

    public func optimization(id: String) -> Optimization? {
        optimizations.first { $0.id == id }
    }

    public func visibleOptimizations(_ filter: OptimizationFilter) -> [Optimization] {
        optimizations.filter { filter.includes($0) }
    }

    func optimizationID(ofAgent agentID: String) -> String? {
        details.first { $0.value.agents.contains { $0.id == agentID } }?.key
    }

    // MARK: Details

    public func loadDetail(id: String) async {
        do {
            details[id] = try await client.optimization(id: id)
            remember(id)
        } catch {
            errors.failLoading(error)
        }
    }

    /// Reads every open detail again, four at a time.
    func reloadDetails() async {
        let ids = Array(details.keys)
        let client = self.client
        let loaded = await Bounded.map(ids, limit: 4) { id -> (String, OptimizationDetail)? in
            guard let detail = try? await client.optimization(id: id) else { return nil }
            return (id, detail)
        }
        for case let (id, detail)? in loaded {
            details[id] = detail
        }
    }

    private func remember(_ id: String) {
        detailOrder.removeAll { $0 == id }
        detailOrder.append(id)
        let live = Set(optimizations.filter(\.isLive).map(\.id))
        for victim in Eviction.victims(order: detailOrder, keeping: live.union([id]), limit: Self.detailLimit) {
            details[victim] = nil
            detailOrder.removeAll { $0 == victim }
        }
    }

    // MARK: Actions

    /// The kill switch. Reloads the optimization so the screen shows how it ended.
    public func stopOptimization(_ id: String) async {
        errors.beginAction()
        do {
            try await client.stopOptimization(id: id)
            await refreshOptimization(id)
        } catch {
            errors.fail(error, "Couldn't stop the optimization")
        }
    }

    /// Opens the pull request for the verified wins. Returns where it is, or nil (and the error line) when it could not.
    public func openPullRequest(_ id: String) async -> URL? {
        errors.beginAction()
        do {
            let url = try await client.openPullRequest(optimizationID: id)
            await refreshOptimization(id)
            return url
        } catch {
            errors.fail(error, "Couldn't open the pull request")
            return nil
        }
    }

    public func report(_ id: String) async -> OptimizationReport? {
        errors.beginAction()
        do {
            return try await client.report(optimizationID: id)
        } catch {
            errors.fail(error)
            return nil
        }
    }

    private func refreshOptimization(_ id: String) async {
        await loadDetail(id: id)
        if let fresh = details[id]?.optimization { onChange?(fresh) }
    }

    public func pauseAgent(_ id: String) async {
        errors.beginAction()
        do {
            let agent = try await client.pauseAgent(id: id)
            if let optimizationID = optimizationID(ofAgent: id) { onEvent?(.agentUpdated(optimizationID: optimizationID, agent: agent)) }
        } catch {
            errors.fail(error)
        }
    }

    public func resumeAgent(_ id: String) async {
        errors.beginAction()
        do {
            let agent = try await client.resumeAgent(id: id)
            if let optimizationID = optimizationID(ofAgent: id) { onEvent?(.agentUpdated(optimizationID: optimizationID, agent: agent)) }
        } catch {
            errors.fail(error)
        }
    }

    public func messageAgent(_ id: String, text: String) async {
        errors.beginAction()
        do {
            try await client.messageAgent(id: id, text: text)
        } catch {
            errors.fail(error)
        }
    }
}
