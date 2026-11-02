import Foundation

/// A harness: an agent CLI Ratchet drives on this Mac (Claude Code, Codex or Grok). The word "agent" is left for one running
/// worker of an optimization.
public enum Harness: String, CaseIterable, Codable, Sendable, Identifiable {
    case claudeCode = "claude-code"
    case codex
    case grok

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .claudeCode: "Claude Code"
        case .codex: "Codex"
        case .grok: "Grok"
        }
    }

    /// Who issues the API key.
    public var vendor: String {
        switch self {
        case .claudeCode: "Anthropic"
        case .codex: "OpenAI"
        case .grok: "xAI"
        }
    }

    /// The key a pasted API key must look like.
    public var secretKind: SecretKind {
        switch self {
        case .claudeCode: .anthropicKey
        case .codex: .openaiKey
        case .grok: .xaiKey
        }
    }

    /// The agent named in free text ("Codex · team key"); Claude Code when it names none.
    public init(named name: String) {
        let lower = name.lowercased()
        self = lower.contains("codex") ? .codex : lower.contains("grok") ? .grok : .claudeCode
    }
}

/// One model a harness can run: the id or alias its CLI takes and the name people see ("Opus").
public struct HarnessModel: Identifiable, Hashable, Codable, Sendable {
    public var id: String
    public var title: String
    public var harness: Harness

    public init(id: String, title: String, harness: Harness) {
        self.id = id
        self.title = title
        self.harness = harness
    }
}

/// What a person picked for a conversation or a run: a harness and, optionally, one of its models (nil is its default).
public struct HarnessChoice: Hashable, Codable, Sendable {
    public var harness: Harness
    public var model: HarnessModel?

    public init(harness: Harness, model: HarnessModel? = nil) {
        self.harness = harness
        self.model = model
    }

    /// "Claude Code" or "Claude Code · Opus".
    public var title: String { model.map { harness.title + " · " + $0.title } ?? harness.title }
}

/// The harnesses found on this Mac with their models: the one place that says which can be chosen and what the default is.
public struct HarnessCatalog: Sendable, Equatable {
    /// Every harness Ratchet knows, in the order a default is looked for.
    public static let order: [Harness] = [.claudeCode, .codex, .grok]

    public var detected: [DetectedHarness]

    public init(_ detected: [DetectedHarness] = []) {
        self.detected = detected
    }

    public static let empty = HarnessCatalog()

    public func detection(of harness: Harness) -> DetectedHarness? {
        detected.first { $0.harness == harness }
    }

    /// Installed and signed in (or holding an API key): a conversation or a run can start on it.
    public func isReady(_ harness: Harness) -> Bool {
        guard let found = detection(of: harness) else { return false }
        return found.installed && found.auth != .signedOut
    }

    public var ready: [Harness] { Self.order.filter(isReady) }

    public func models(of harness: Harness) -> [HarnessModel] {
        detection(of: harness)?.models ?? []
    }

    /// The first ready harness, with its default model.
    public var defaultChoice: HarnessChoice? {
        guard let harness = ready.first else { return nil }
        return choice(for: harness)
    }

    /// `harness` with its default model (nil model when the daemon named none).
    public func choice(for harness: Harness) -> HarnessChoice {
        let list = models(of: harness)
        let wanted = detection(of: harness)?.defaultModel
        let model = list.first { $0.id == wanted } ?? list.first
        return HarnessChoice(harness: harness, model: model)
    }

    /// One choice per ready harness.
    public var choices: [HarnessChoice] { ready.map(choice(for:)) }
}
