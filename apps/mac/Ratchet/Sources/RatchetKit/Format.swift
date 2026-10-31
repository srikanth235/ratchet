import Foundation

// Number formatting for the whole app. All of it is pure and locale-independent, so the snapshots and the tests
// always read the same: "$184", "$15.4k", "−30%", "1 h 12 m".

public enum Money {
    /// "$0.59", "$38", "$184", "$4.8k", "$15.4k". Whole dollars from $10, one decimal in thousands.
    public static func short(_ value: Double) -> String {
        if value < 0 { return "−" + short(-value) }
        if value >= 100_000 { return String(format: "$%.0fk", value / 1000) }
        if value >= 1000 { return String(format: "$%.1fk", value / 1000) }
        if value >= 10 { return String(format: "$%.0f", value) }
        return String(format: "$%.2f", value)
    }

    /// Dollars and cents: "$1.18".
    public static func exact(_ value: Double) -> String {
        value < 0 ? "−" + exact(-value) : String(format: "$%.2f", value)
    }

    /// "$0.36/h".
    public static func perHour(_ value: Double) -> String {
        exact(value) + "/h"
    }

    /// Chart axis labels: "$0", "$5k", "$12.5k".
    public static func axis(_ value: Double) -> String {
        if value == 0 { return "$0" }
        if value < 1000 { return String(format: "$%.0f", value) }
        let k = value / 1000
        return k == k.rounded() ? String(format: "$%.0fk", k) : String(format: "$%.1fk", k)
    }
}

public enum Percent {
    /// A signed change: "−30%", "+5.9%", "0%". One decimal below 10%.
    public static func signed(_ fraction: Double) -> String {
        let percent = fraction * 100
        let magnitude = abs(percent)
        let decimals = magnitude >= 9.95 ? 0 : 1
        let body = String(format: "%.\(decimals)f", magnitude)
        if Double(body) == 0 { return "0%" }
        return (percent < 0 ? "−" : "+") + body + "%"
    }

    /// "37%".
    public static func plain(_ fraction: Double, decimals: Int = 0) -> String {
        String(format: "%.\(decimals)f%%", fraction * 100)
    }

    /// "±2.1%", "±2%".
    public static func noise(_ fraction: Double) -> String {
        let percent = abs(fraction * 100)
        if percent >= 9.95 || percent == percent.rounded() { return String(format: "±%.0f%%", percent) }
        return String(format: "±%.1f%%", percent)
    }
}

public enum Elapsed {
    /// "1 h 12 m", "2 h", "45 m", "30 s".
    public static func hm(_ seconds: Double) -> String {
        let total = max(Int(seconds.rounded()), 0)
        let hours = total / 3600
        let minutes = (total % 3600) / 60
        if hours > 0 { return minutes > 0 ? "\(hours) h \(minutes) m" : "\(hours) h" }
        if minutes > 0 { return "\(minutes) m" }
        return "\(total) s"
    }

    /// "4 days", "1 day", "under a day".
    public static func days(_ value: Double) -> String {
        let n = Int(value.rounded())
        if value < 0.5 { return "under a day" }
        return n == 1 ? "1 day" : "\(n) days"
    }

    /// "6 min".
    public static func minutes(_ value: Double) -> String {
        "\(Int(value.rounded())) min"
    }

    /// "15:40", in the given time zone (the current one by default).
    public static func timeOfDay(_ date: Date, timeZone: TimeZone = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timeZone
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: date)
    }

    /// "Oct 31".
    public static func shortDate(_ date: Date, timeZone: TimeZone = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timeZone
        formatter.dateFormat = "MMM d"
        return formatter.string(from: date)
    }
}

public enum Figure {
    /// "84×".
    public static func multiple(_ value: Double) -> String {
        value >= 10 ? String(format: "%.0f×", value) : String(format: "%.1f×", value)
    }

    /// "9 verified wins", "1 verified win".
    public static func wins(_ count: Int) -> String {
        count == 1 ? "1 verified win" : "\(count) verified wins"
    }

    /// "$38 / 50": spent against a budget, the budget without its dollar sign.
    public static func spentOf(_ spent: Double, budget: Double?) -> (spent: String, budget: String?) {
        guard let budget else { return (Money.short(spent), nil) }
        return (Money.short(spent), String(format: "%.0f", budget))
    }
}
