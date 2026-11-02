import Foundation

/// Why a turn failed, in a form the UI can act on. The daemon writes `title` and `message` for people and lists the buttons.
public enum ChatErrorCode: String, Codable, Sendable, Equatable {
    case rateLimited = "rate_limited"
    case authExpired = "auth_expired"
    case unavailable
    case failed
}

/// A button the error offers.
public enum ChatFix: String, Codable, Sendable, Equatable {
    case retry
    case signIn = "sign-in"
    case switchHarness = "switch-harness"
}

/// When a rate-limited harness can be tried again.
public struct ChatRetry: Hashable, Sendable {
    public var canRetryNow: Bool
    public var resetsAt: Date?

    public init(canRetryNow: Bool, resetsAt: Date? = nil) {
        self.canRetryNow = canRetryNow
        self.resetsAt = resetsAt
    }
}

/// A chat turn that failed: typed, so the view shows "Claude Code hit its limit. Try again at 15:40" with the right buttons
/// instead of one string.
public struct ChatError: Error, Hashable, Sendable, LocalizedError {
    public var code: ChatErrorCode
    public var harness: Harness
    public var title: String
    public var message: String
    public var fixes: [ChatFix]
    public var retry: ChatRetry?
    /// The harness the "switch" fix offers.
    public var switchTo: Harness?

    public init(
        code: ChatErrorCode, harness: Harness, title: String, message: String, fixes: [ChatFix] = [],
        retry: ChatRetry? = nil, switchTo: Harness? = nil
    ) {
        self.code = code
        self.harness = harness
        self.title = title
        self.message = message
        self.fixes = fixes
        self.retry = retry
        self.switchTo = switchTo
    }

    public var errorDescription: String? { message.isEmpty ? title : title + " " + message }

    public var isRateLimited: Bool { code == .rateLimited }
    public var isAuthExpired: Bool { code == .authExpired }

    /// "Try again" is worth showing: the harness offers it, and a rate limit has passed (or never had a reset time).
    public func canRetry(at now: Date) -> Bool {
        guard fixes.contains(.retry) else { return false }
        guard let retry else { return true }
        if retry.canRetryNow { return true }
        guard let resetsAt = retry.resetsAt else { return true }
        return resetsAt <= now
    }

    /// The line under the title when the harness is rate limited and says when it resets: "Resets at 15:40".
    public func resetNote(now: Date) -> String? {
        guard isRateLimited, let resetsAt = retry?.resetsAt, resetsAt > now else { return nil }
        return "Resets at " + Elapsed.timeOfDay(resetsAt)
    }
}
