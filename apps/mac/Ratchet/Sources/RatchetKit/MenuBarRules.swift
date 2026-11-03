import Foundation

/// The words and numbers of the menu bar panel, as pure functions, so tests cover them without a window.
public enum MenuBarRules {
    /// How much of the time limit is used, 0 to 1 (0 when the limit or the time left is not known).
    public static func timeFraction(_ optimization: Optimization) -> Double {
        let total = optimization.timeLimitHours * 3600
        guard total > 0, let left = optimization.secondsLeft else { return 0 }
        return min(max(1 - left / total, 0), 1)
    }

    /// "1h 20m left · $12 spent".
    public static func runNote(_ optimization: Optimization) -> String {
        var parts: [String] = []
        if let left = optimization.secondsLeft { parts.append(Elapsed.hm(left) + " left") }
        parts.append(Money.short(optimization.spentUSD) + " spent")
        return parts.joined(separator: " · ")
    }

    /// "1 needs you", "3 need you": the count next to the menu bar icon's name and in the panel's header.
    public static func needsYouText(_ count: Int) -> String {
        "\(count) " + (count == 1 ? "needs" : "need") + " you"
    }

    /// The sandbox line at the bottom of the panel.
    public static func sandboxLine(_ sandbox: SandboxStatus) -> String {
        guard sandbox.isOn else { return "Sandbox off" }
        var line = "Sandbox on · " + (sandbox.account ?? "AWS") + " · " + Money.exact(sandbox.costSoFarUSD) + " so far"
        if let offAt = sandbox.offAt { line += " · off at " + Elapsed.timeOfDay(offAt) }
        return line
    }
}
