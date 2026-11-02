import Foundation

public enum Histogram {
    /// How many bins a histogram of `count` samples gets: about 4 per square root, between 8 and 28.
    public static func binCount(forSamples count: Int) -> Int {
        max(8, min(28, Int((Double(max(count, 1)).squareRoot() * 4).rounded())))
    }

    /// Counts every finite sample in `lo...hi`; the maximum goes into the last bin.
    public static func bins(of values: [Double], lo: Double, hi: Double, count: Int) -> [Int] {
        var out = [Int](repeating: 0, count: max(count, 1))
        let span = hi - lo == 0 ? 1 : hi - lo
        for value in values where value.isFinite {
            let index = Int(((value - lo) / span * Double(out.count)).rounded(.down))
            out[min(out.count - 1, max(0, index))] += 1
        }
        return out
    }

    /// The shared range of the series, so two histograms overlay on one x axis; nil without any finite sample.
    public static func range(of series: [[Double]]) -> ClosedRange<Double>? {
        let all = series.flatMap { $0 }.filter(\.isFinite)
        guard let low = all.min(), let high = all.max() else { return nil }
        return low...high
    }
}

public enum EvalBars {
    /// Heights from 6 to `height`, scaled over the value range (all equal values get 60%). Blocked bars are drawn as a flat
    /// stub by the view; this is only the scaling.
    public static func heights(of values: [Double], height: Double) -> [Double] {
        guard let low = values.min(), let high = values.max() else { return [] }
        let span = high - low
        return values.map { value in
            max(6, span == 0 ? (height * 0.6).rounded() : (6 + (value - low) / span * (height - 12)).rounded())
        }
    }

    /// Where a dashed guide for `best` sits inside bars scaled by `heights(of:height:)`, or nil when all values are equal.
    public static func guide(for best: Double, values: [Double], height: Double) -> Double? {
        guard let low = values.min(), let high = values.max(), high != low else { return nil }
        return 6 + (best - low) / (high - low) * (height - 12)
    }
}
