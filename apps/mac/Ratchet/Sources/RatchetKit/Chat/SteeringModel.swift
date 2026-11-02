import Foundation
import Observation

/// Where a steering note stands.
public enum SteeringStatus: Equatable, Sendable {
    /// Sent; an agent reads it at its next candidate. Can be taken back.
    case queued
    /// An agent took it up.
    case read(by: String?, at: Date?)
    /// Taken back before anybody read it.
    case withdrawn
}

/// One note on the timeline: "You · “focus on database queries” Queued · read at the next candidate  Undo", then
/// "Read by Claude Code at 02:11".
public struct SteeringEntry: Identifiable, Equatable, Sendable {
    public var id: String
    public var text: String
    public var status: SteeringStatus
    /// The line beside the words, in the words of the mockup.
    public var statusLine: String
    /// Undo is there until an agent reads the note.
    public var canUndo: Bool { status == .queued }

    init(card: ChatCard, timeZone: TimeZone) {
        id = card.id
        text = card.title
        switch card.status {
        case .read:
            status = .read(by: card.readBy, at: card.readAt)
        case .replaced, .failed:
            status = .withdrawn
        default:
            status = .queued
        }
        switch status {
        case .queued:
            statusLine = "Queued · read at the next candidate"
        case .read(let by, let at):
            var line = "Read"
            if let by { line += " by " + by }
            if let at { line += " at " + Elapsed.timeOfDay(at, timeZone: timeZone) }
            statusLine = line
        case .withdrawn:
            statusLine = "Withdrawn"
        }
    }
}

/// Steering: a chat bound to one optimization. What the person writes becomes a note, queued until an agent reads it between
/// candidates (then `read`, with the agent and the time), and can be taken back until then. The composer is multi-line.
@MainActor @Observable
public final class SteeringModel {
    public let optimizationID: String
    public private(set) var chatID: String?
    public var draft = ""
    /// Why the chat could not open or a note could not be sent.
    public private(set) var failure: String?

    @ObservationIgnored private let store: ChatStore
    @ObservationIgnored private let timeZone: TimeZone

    public init(store: ChatStore, optimizationID: String, timeZone: TimeZone = .current) {
        self.store = store
        self.optimizationID = optimizationID
        self.timeZone = timeZone
    }

    /// The composer's hint: "Return adds a line · ⌘↩ sends".
    public var hint: String { ComposerKey.hint(multiline: true) }
    public static let placeholder = "Leave a note to steer the next candidate, e.g. “focus on database queries”. Notes are read between candidates."

    /// Opens the chat for this optimization (once); the id is what `ChatStore.follow` keeps current.
    @discardableResult
    public func open() async -> String? {
        if let chatID { return chatID }
        guard let id = await store.start(NewChat(optimization: optimizationID)) else {
            failure = store.startError
            return nil
        }
        chatID = id
        failure = nil
        return id
    }

    private var session: ChatSession? { chatID.flatMap { store.session($0) } }

    /// The notes, oldest first, newest state of each.
    public var entries: [SteeringEntry] {
        (session?.cards ?? []).filter { $0.kind == .note }.map { SteeringEntry(card: $0, timeZone: timeZone) }
    }

    public var canSend: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !(session?.isReplying ?? false)
    }

    /// Sends the note. The words stay in the composer when it could not be sent.
    public func send() async {
        guard canSend else { return }
        guard let id = await open() else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        draft = ""
        await store.send(id, text: text)
        if let error = store.session(id)?.actionError {
            failure = error
            draft = text
        } else {
            failure = nil
        }
    }

    /// Takes a note back; refused once an agent has read it.
    public func undo(_ entryID: String) async {
        guard let id = chatID, entries.first(where: { $0.id == entryID })?.canUndo == true else { return }
        await store.withdrawNote(id, noteID: entryID)
    }
}
