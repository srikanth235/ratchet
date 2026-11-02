import Foundation

/// A long table shows its first rows and a "Show all" button instead of laying out thousands at once.
public struct RowCap: Sendable, Equatable {
    /// Rows shown before "Show all".
    public static let limit = 200

    public var showsAll = false

    public init(showsAll: Bool = false) {
        self.showsAll = showsAll
    }

    /// The rows to lay out.
    public func shown<Row>(_ rows: [Row]) -> ArraySlice<Row> {
        showsAll ? rows[...] : rows.prefix(Self.limit)
    }

    /// How many rows are held back; 0 when all are shown.
    public func hidden(_ count: Int) -> Int {
        showsAll ? 0 : max(count - Self.limit, 0)
    }

    /// "Show all 1,204 rows", or nil when nothing is held back.
    public func buttonTitle(count: Int, noun: String = "rows") -> String? {
        hidden(count) > 0 ? "Show all \(count) \(noun)" : nil
    }
}
