import Foundation

/// The format checks of the Connectors and Advanced forms, run while typing and again by the daemon before anything is
/// stored. `nil` means valid. Empty values are valid for settings (they mean "not set").
public enum SettingsCheck {
    private static func matches(_ text: String, _ pattern: String) -> Bool {
        text.range(of: pattern, options: .regularExpression) != nil
    }

    private static func isAbsolute(_ path: String) -> Bool {
        path.hasPrefix("/") || path.hasPrefix("~/")
    }

    /// A path or a bare command name: no shell metacharacters, no control characters.
    private static func isPathLike(_ text: String) -> Bool {
        let forbidden: Set<Character> = [";", "&", "|", "`", "$", "<", ">", "*", "?", "\"", "'", "\\", "\0", "\r", "\n"]
        return !text.isEmpty && !text.contains { forbidden.contains($0) }
    }

    public static func problem(for field: AdvancedField, _ raw: String) -> String? {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        switch field {
        case .runnerAMI, .benchAMI:
            return value.isEmpty || matches(value, "^ami-([0-9a-f]{8}|[0-9a-f]{17})$") ? nil : "Use an AMI id such as ami-0123456789abcdef0"
        case .tofuBinary:
            if value.isEmpty { return nil }
            if value.count > 500 || !isPathLike(value) || value.hasPrefix("-") {
                return "Use a path or command name without quotes or shell characters"
            }
            return value.contains(where: \.isWhitespace) && !isAbsolute(value)
                ? "Use an absolute path (or just \"tofu\" for the one on PATH)" : nil
        case .moduleDir:
            if value.isEmpty { return nil }
            if value.count > 500 || !isPathLike(value) || value.hasPrefix("-") {
                return "Use a directory path without quotes or shell characters"
            }
            return isAbsolute(value) ? nil : "Use an absolute path"
        }
    }

    /// The first problem among the four overrides, or nil.
    public static func problems(in settings: AdvancedSettings) -> [AdvancedField: String] {
        var found: [AdvancedField: String] = [:]
        for field in AdvancedField.allCases {
            if let problem = problem(for: field, settings.value(of: field)) { found[field] = problem }
        }
        return found
    }

    public static func regionProblem(_ raw: String) -> String? {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty || matches(value, "^[a-z]{2}(-[a-z]+)+-[0-9]$") ? nil : "Use a region id such as us-east-1 or eu-west-2"
    }

    /// The monthly cap typed as text: empty is "no cap".
    public static func cap(_ raw: String) -> (value: Double?, problem: String?) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return (nil, nil) }
        guard let number = Double(text), number.isFinite, number > 0 else { return (nil, "Use a positive number of dollars") }
        return (number, nil)
    }
}

/// Which secret a pasted value must look like.
public enum SecretKind: String, Sendable {
    case anthropicKey, openaiKey, xaiKey, githubToken

    /// Loose prefix checks that catch the common paste mistakes (the wrong key in the wrong box, stray spaces).
    public func problem(_ value: String) -> String? {
        if value != value.trimmingCharacters(in: .whitespacesAndNewlines) || value.contains(where: \.isWhitespace) {
            return "Remove spaces and line breaks around the value"
        }
        func matches(_ pattern: String) -> Bool { value.range(of: pattern, options: .regularExpression) != nil }
        switch self {
        case .anthropicKey:
            return matches("^sk-ant-[A-Za-z0-9_-]{8,}$") ? nil : "Anthropic keys start with sk-ant-"
        case .openaiKey:
            return matches("^sk-(?!ant-)[A-Za-z0-9_-]{16,}$") ? nil : "OpenAI keys start with sk- (not sk-ant-) and are at least 20 characters"
        case .xaiKey:
            return matches("^xai-[A-Za-z0-9_-]{16,}$") ? nil : "xAI keys start with xai-"
        case .githubToken:
            return matches("^(gh[pousr]_[A-Za-z0-9]{20,255}|github_pat_[A-Za-z0-9_]{20,255}|[0-9a-f]{40})$")
                ? nil : "GitHub tokens start with ghp_, gho_, ghu_, ghs_ or github_pat_"
        }
    }

    /// Long enough to try, and without a format problem: the Add button's condition.
    public func isAcceptable(_ value: String) -> Bool {
        value.count >= 8 && problem(value) == nil
    }
}
