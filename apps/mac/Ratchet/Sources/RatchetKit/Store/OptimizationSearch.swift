import Foundation

/// The page-scoped search of the Overview and Optimizations tables: every word of the query must appear in the title, the repo,
/// the goal or the status, in any order, ignoring case and accents. An empty query keeps everything.
public enum OptimizationSearch {
    public static func filter(_ optimizations: [Optimization], query: String) -> [Optimization] {
        let words = query.split(whereSeparator: \.isWhitespace).map(String.init)
        guard !words.isEmpty else { return optimizations }
        return optimizations.filter { optimization in
            words.allSatisfy { word in
                optimization.title.localizedStandardContains(word)
                    || optimization.repo.localizedStandardContains(word)
                    || optimization.goal.localizedStandardContains(word)
                    || optimization.status.title.localizedStandardContains(word)
            }
        }
    }
}
