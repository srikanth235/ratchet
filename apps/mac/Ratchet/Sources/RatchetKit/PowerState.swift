import Foundation

/// Whether the daemon holds the prevent-sleep assertion: exactly while a run whose agents run on this Mac is live. A run in the
/// cloud does not count; the Mac may sleep under it.
public struct PowerState: Hashable, Sendable {
    public var keepingAwake: Bool
    /// The optimizations running on this Mac.
    public var localOptimizationIDs: [String]

    public init(keepingAwake: Bool = false, localOptimizationIDs: [String] = []) {
        self.keepingAwake = keepingAwake
        self.localOptimizationIDs = localOptimizationIDs
    }

    /// The menu bar line; nil while the Mac may sleep.
    public var menuLine: String? { keepingAwake ? "Keeping your Mac awake" : nil }

    /// The line under it: "While 2 optimizations run on this Mac."
    public var menuDetail: String? {
        guard keepingAwake else { return nil }
        let count = localOptimizationIDs.count
        guard count > 0 else { return nil }
        return "While \(Plural.count(count, "optimization")) run\(count == 1 ? "s" : "") on this Mac."
    }
}
