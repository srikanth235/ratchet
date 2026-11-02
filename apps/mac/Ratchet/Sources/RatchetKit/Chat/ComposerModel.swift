import Foundation
import Observation

/// The chips under the Home composer. Each opens a popover.
public enum ComposerChip: String, CaseIterable, Identifiable, Sendable {
    case repo, cloud, agents, limits, schedule

    public var id: String { rawValue }

    /// The popover's title ("Change agents"), also the accessibility label of the chip.
    public var popoverTitle: String {
        switch self {
        case .repo: "Choose repo"
        case .cloud: "Choose cloud"
        case .agents: "Change agents"
        case .limits: "Change limits"
        case .schedule: "Change schedule"
        }
    }
}

/// What the composer can offer, from the rest of the app: repos, AWS accounts and the harnesses on this Mac.
public struct ComposerOptions: Equatable, Sendable {
    public var repos: [String]
    public var cloudAccounts: [String]
    public var catalog: HarnessCatalog

    public init(repos: [String] = [], cloudAccounts: [String] = [], catalog: HarnessCatalog = .empty) {
        self.repos = repos
        self.cloudAccounts = cloudAccounts
        self.catalog = catalog
    }
}

/// The Home composer: the words, five chips (repo, cloud, agents, limits, schedule) as popover models, and Send. Sending starts
/// a chat on the first agent of the list and says the words plus whatever the chips set that the words do not already say.
@MainActor @Observable
public final class ComposerModel {
    public var text = ""
    public var repo: String?
    public var cloudAccount: String?
    public var agents: AgentOrderEditor
    /// nil until the person sets them: the chat then uses the defaults for new optimizations.
    public var limits: LimitsEditor?
    public var schedule: ScheduleKind?
    public var options: ComposerOptions

    public private(set) var isSending = false
    /// Why the chat could not start ("No agent is signed in…"); the words stay in the composer.
    public private(set) var failure: String?

    @ObservationIgnored private let store: ChatStore

    public init(store: ChatStore, options: ComposerOptions = ComposerOptions()) {
        self.store = store
        self.options = options
        let ready = options.catalog.ready
        self.agents = AgentOrderEditor(order: ready.isEmpty ? [.claudeCode] : ready, placement: .local, available: ready.isEmpty ? Harness.allCases : ready)
    }

    /// Refreshes what the chips offer (the catalog changed after a sign-in) and keeps the person's choices that still exist.
    public func update(options new: ComposerOptions) {
        options = new
        let ready = new.catalog.ready
        guard !ready.isEmpty else { return }
        var editor = AgentOrderEditor(order: agents.order.filter { ready.contains($0) }, placement: agents.placement, available: ready)
        if editor.order.isEmpty { editor.add(ready[0]) }
        agents = editor
    }

    // MARK: Chips

    /// The text on a chip.
    public func title(of chip: ComposerChip) -> String {
        switch chip {
        case .repo: repo ?? "Choose repo"
        case .cloud: cloudAccount.map { $0 + " · AWS" } ?? "No cloud"
        case .agents: agents.chipTitle
        case .limits: limits.map { Money.short(Double($0.budgetUSD)) + " · " + Elapsed.hm($0.hours * 3600) + " per run" } ?? "Default limits"
        case .schedule: schedule?.title ?? "Not scheduled"
        }
    }

    /// A chip the person set (drawn with the accent) as opposed to a default.
    public func isSet(_ chip: ComposerChip) -> Bool {
        switch chip {
        case .repo: repo != nil
        case .cloud: cloudAccount != nil
        case .agents: true
        case .limits: limits != nil
        case .schedule: schedule != nil
        }
    }

    // MARK: Sending

    /// Return and Command-Return send here; there is one line of text.
    public static let hint = ComposerKey.hint(multiline: false)

    public var canSend: Bool {
        !isSending && !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !options.catalog.ready.isEmpty
    }

    /// Why Send is disabled when there is something to send: no agent is signed in.
    public var blockedReason: String? {
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return nil }
        return options.catalog.ready.isEmpty ? "No agent is signed in. Sign in to Claude Code, Codex or Grok first." : nil
    }

    /// The first message: the words, then what the chips add that the words do not say.
    public func message() -> String {
        var message = text.trimmingCharacters(in: .whitespacesAndNewlines)
        var extras: [String] = []
        let lower = message.lowercased()
        if let limits, !lower.contains("$") { extras.append("Each run may use up to \(Money.short(Double(limits.budgetUSD))) and \(Elapsed.hm(limits.hours * 3600)).") }
        if let schedule, !Self.mentionsSchedule(lower) { extras.append(schedule.instruction) }
        if agents.order.count > 1 { extras.append(agents.instruction) } else if agents.placement == .cloud { extras.append("Run it in the cloud.") }
        if let cloudAccount { extras.append("Use the \(cloudAccount) AWS account.") }
        if !extras.isEmpty { message += (message.hasSuffix(".") || message.hasSuffix("?") || message.hasSuffix("!") ? " " : ". ") + extras.joined(separator: " ") }
        return message
    }

    private static func mentionsSchedule(_ lower: String) -> Bool {
        ["every night", "nightly", "weekday", "every week", "weekly", "on every push", "run it once"].contains { lower.contains($0) }
    }

    /// Starts the chat and says the first message. Returns the chat's id (the app shows the conversation), or nil with `failure`
    /// set; the words stay in the composer then. The first agent of the list is the chat's harness.
    @discardableResult
    public func send() async -> String? {
        guard canSend else { return nil }
        isSending = true
        defer { isSending = false }
        let harness = agents.order.first
        let choice = harness.map { options.catalog.choice(for: $0) }
        guard let id = await store.start(NewChat(repo: repo, harness: harness, model: choice?.model?.id)) else {
            failure = store.startError
            return nil
        }
        let words = message()
        await store.send(id, text: words)
        if let error = store.session(id)?.actionError {
            failure = error
            await store.end(id)
            return nil
        }
        failure = nil
        text = ""
        return id
    }

    /// Start Over brings the prompt back.
    public func restore(prompt: String) {
        text = prompt
    }
}
