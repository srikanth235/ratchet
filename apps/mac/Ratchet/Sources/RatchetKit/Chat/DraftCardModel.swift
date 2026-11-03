import Foundation
import Observation

/// One row of the draft card as the view draws it.
public struct DraftRowState: Identifiable, Equatable, Sendable {
    public var row: ChatCardRow
    /// "Change agents": every row button has its own label.
    public var changeTitle: String
    /// "Updated" shows beside a row the person just changed, with Undo.
    public var isUpdated: Bool

    public var id: String { row.key }
    public var isEditable: Bool { row.editable }
    public var canUndo: Bool { isUpdated }
}

/// A fix under a failed draft ("Couldn't lock the environment").
public enum DraftFix: Equatable, Sendable, Identifiable {
    case useSmallerSetup
    case tryAgain

    public var id: String { title }

    public var title: String {
        switch self {
        case .useSmallerSetup: "Use Smaller Setup"
        case .tryAgain: "Try Again"
        }
    }

    var instruction: String {
        switch self {
        case .useSmallerSetup: "Use a smaller setup."
        case .tryAgain: "Try the environment check again."
        }
    }
}

/// The draft card of a creation conversation: its rows with a per-row edit model and Updated / Undo, Start Over, Create and
/// Create and Run Once with its cost. Every action is a message to the chat (the chat drafts again and the card updates), the
/// same as saying it in words: creating from the card and saying "create it" are one call.
@MainActor @Observable
public final class DraftCardModel {
    public let chatID: String

    /// What one run typically costs, for "Create and Run Once · ≈$38" (from the Overview forecast); nil falls back to the
    /// run's budget cap ("up to $50").
    public var typicalRunCostUSD: Double?
    /// The AWS account the draft runs its sandbox in, as the Home chip chose it; nil when none was (the agents run on this Mac).
    public var cloudAccount: String?
    /// The accounts the Cloud row offers; the row is not editable while there are none.
    public var cloudAccounts: [String] = []
    /// Rows the person changed, with the row as it was: Undo puts it back.
    public private(set) var changes: [String: ChatCardRow] = [:]

    @ObservationIgnored private let store: ChatStore

    public init(store: ChatStore, chatID: String, typicalRunCostUSD: Double? = nil) {
        self.store = store
        self.chatID = chatID
        self.typicalRunCostUSD = typicalRunCostUSD
    }

    private var session: ChatSession? { store.session(chatID) }

    // MARK: What is shown

    /// The draft being talked about: the newest one that has not been replaced.
    public var card: ChatCard? {
        session?.cards.last { $0.kind == .draft && $0.status != .replaced }
    }

    public var status: ChatCardStatus? { card?.status }
    public var title: String { card?.title ?? "" }
    public var subtitle: String? { card?.subtitle }

    /// The setup agent is writing a new draft: the composer shows Stop.
    public var isDrafting: Bool { session?.isReplying ?? false }

    /// What the card lists: the daemon's rows, with Repository and then Cloud after Agents whenever the daemon did not send them, so
    /// every draft says where it reads from and where it runs (the same two as the Home chips).
    public var allRows: [ChatCardRow] {
        var list = card?.rows ?? []
        var at: Int = list.count
        if let agentsAt = list.firstIndex(where: { $0.key == "agents" }) { at = agentsAt + 1 }
        if !list.contains(where: { $0.key == "repo" }) {
            let repo = card?.repo ?? session?.core.chat.repo
            list.insert(ChatCardRow(key: "repo", label: "Repository", value: repo ?? "Not chosen"), at: at)
            at += 1
        }
        if !list.contains(where: { $0.key == "cloud" }) {
            list.insert(
                ChatCardRow(
                    key: "cloud", label: "Cloud", value: cloudAccount.map { $0 + " · AWS" } ?? "None · agents run on this Mac",
                    editable: !cloudAccounts.isEmpty, options: cloudAccounts.map { ChatCardOption(label: $0, value: $0) }),
                at: at)
        }
        return list
    }

    public var rows: [DraftRowState] {
        allRows.map { row in
            let before = changes[row.key]
            return DraftRowState(
                row: row, changeTitle: "Change " + Self.noun(of: row), isUpdated: before != nil && before?.value != row.value)
        }
    }

    /// "agents", "limits", "measure": the word on the row's button.
    static func noun(of row: ChatCardRow) -> String {
        switch row.key {
        case "limits": "limits"
        case "measure": "measure"
        default: row.label.lowercased()
        }
    }

    /// The failed environment check, in words: title and message.
    public var failure: (title: String, message: String)? {
        guard let card, card.status == .failed else { return nil }
        return (card.errorTitle ?? "Couldn't create this optimization", card.errorMessage ?? "")
    }

    public var fixes: [DraftFix] { failure == nil ? [] : [.useSmallerSetup, .tryAgain] }

    /// Both create buttons: only on a draft that is ready, while the chat is not busy writing another.
    public var canCreate: Bool { card?.status == .draft && !isDrafting }

    public var createTitle: String { "Create Optimization" }

    /// "Create and Run Once · ≈$38", or "· up to $50" before there is a typical cost.
    public var createAndRunOnceTitle: String {
        "Create and Run Once · " + runOnceCostText
    }

    public var runOnceCostText: String {
        if let typical = typicalRunCostUSD { return "≈" + Money.short(typical) }
        if let budget = card?.rows.first(where: { $0.key == "limits" }).map(LimitsEditor.init(row:))?.budgetUSD {
            return "up to " + Money.short(Double(budget))
        }
        return "cost shown after the first run"
    }

    /// The optimization the draft became, once it was created: the app opens it.
    public var createdOptimizationID: String? { card?.status == .created ? card?.optimization : nil }

    /// The editor a row's popover starts from; nil for a row that is not editable.
    public func editor(for key: String) -> DraftRowEdit? {
        guard let row = allRows.first(where: { $0.key == key }), row.editable else { return nil }
        switch key {
        case "agents": return .agents(AgentOrderEditor(row: row))
        case "limits": return .limits(LimitsEditor(row: row))
        case "schedule": return .schedule(ScheduleKind(shown: row.value) ?? .once)
        default: return row.options.first.map { .option(rowLabel: row.label, option: $0) } ?? .text(rowLabel: row.label, "")
        }
    }

    // MARK: Actions

    /// Says the change to the chat and remembers the row as it was, so "Updated" and Undo can show.
    public func edit(_ key: String, _ edit: DraftRowEdit) async {
        guard canCreate || card?.status == .failed, let row = allRows.first(where: { $0.key == key }) else { return }
        changes[key] = row
        await store.send(chatID, text: edit.instruction)
    }

    /// Puts a changed row back: says the old value to the chat. Works only on a row that shows "Updated".
    public func undo(_ key: String) async {
        guard let before = changes[key] else { return }
        changes[key] = nil
        await store.send(chatID, text: DraftRowEdit.restoring(before).instruction)
    }

    public func create() async {
        guard canCreate else { return }
        await store.send(chatID, text: "Create it.")
    }

    /// Creates it and starts one run, without adding a schedule.
    public func createAndRunOnce() async {
        guard canCreate else { return }
        await store.send(chatID, text: "Create it and run it once. Do not add a schedule.")
    }

    public func apply(_ fix: DraftFix) async {
        await store.send(chatID, text: fix.instruction)
    }

    /// Stop while the setup agent drafts: ends the turn and keeps the steps so far.
    public func stop() async {
        await store.interrupt(chatID)
    }

    /// Ends this conversation and returns what the person first asked, to put back in the Home composer.
    public func startOver() async -> String? {
        let prompt = session?.core.chat.messages.first { $0.role == .user }?.text
        await store.end(chatID)
        return prompt
    }
}
