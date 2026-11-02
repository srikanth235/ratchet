import Foundation

// The app's model of a chat with a harness (`docs/ratchet-chat.md`, the `/v1/chats` routes). No UI uses it yet: the Home
// composer and the conversation view (P5) are built on these types and on `ChatClient`.

public enum ChatState: String, Codable, Sendable, Equatable {
    case idle, replying
}

public enum ChatRole: String, Codable, Sendable, Equatable {
    case user, assistant, tool, card, note
}

public enum ChatBilling: String, Codable, Sendable, Equatable {
    case subscription
    case apiKey = "api-key"
}

public enum ChatOutcome: String, Codable, Sendable, Equatable {
    case completed, interrupted, failed
}

public enum ChatToolState: String, Codable, Sendable, Equatable {
    case running, done, failed
}

/// One step the harness took ("Reading acme/shop").
public struct ChatToolStep: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var name: String
    public var state: ChatToolState
    public var summary: String

    public init(id: String, name: String, state: ChatToolState, summary: String) {
        self.id = id
        self.name = name
        self.state = state
        self.summary = summary
    }
}

/// Tokens of a turn. `costUSD` is 0 on a subscription.
public struct ChatUsage: Codable, Hashable, Sendable {
    public var inputTokens: Int
    public var outputTokens: Int
    public var costUSD: Double?
    public var billing: ChatBilling?

    public init(inputTokens: Int = 0, outputTokens: Int = 0, costUSD: Double? = nil, billing: ChatBilling? = nil) {
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.costUSD = costUSD
        self.billing = billing
    }
}

public enum ChatCardKind: String, Codable, Sendable, Equatable {
    case draft, note
}

public enum ChatCardStatus: String, Codable, Sendable, Equatable {
    case draft, created, failed, replaced, queued, read
}

/// One option of an editable card row; `value` is what a change sends back.
public struct ChatCardOption: Codable, Hashable, Sendable {
    public var label: String
    public var value: String

    public init(label: String, value: String) {
        self.label = label
        self.value = value
    }
}

/// One line of a draft card: "Measures", "Agents", "Each run", "Schedule", "Output", "Config".
public struct ChatCardRow: Identifiable, Codable, Hashable, Sendable {
    public var key: String
    public var label: String
    public var value: String
    public var editable: Bool
    public var options: [ChatCardOption]

    public var id: String { key }

    public init(key: String, label: String, value: String, editable: Bool = false, options: [ChatCardOption] = []) {
        self.key = key
        self.label = label
        self.value = value
        self.editable = editable
        self.options = options
    }
}

/// The card a chat shows for a draft optimization or a steering note. The same `id` arriving again is an update.
public struct ChatCard: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    public var kind: ChatCardKind
    public var status: ChatCardStatus
    public var title: String
    public var subtitle: String?
    public var repo: String?
    /// The optimization a created draft became, or the one a note steers.
    public var optimization: String?
    public var rows: [ChatCardRow]
    /// A failed draft: what went wrong, in words for people.
    public var errorTitle: String?
    public var errorMessage: String?
    /// A steering note: which agent read it, and when.
    public var readBy: String?
    public var readAt: Date?

    public init(
        id: String, kind: ChatCardKind = .draft, status: ChatCardStatus = .draft, title: String, subtitle: String? = nil,
        repo: String? = nil, optimization: String? = nil, rows: [ChatCardRow] = [], errorTitle: String? = nil,
        errorMessage: String? = nil, readBy: String? = nil, readAt: Date? = nil
    ) {
        self.id = id
        self.kind = kind
        self.status = status
        self.title = title
        self.subtitle = subtitle
        self.repo = repo
        self.optimization = optimization
        self.rows = rows
        self.errorTitle = errorTitle
        self.errorMessage = errorMessage
        self.readBy = readBy
        self.readAt = readAt
    }
}

/// One message of a chat. `seq` counts up per chat and is also its identity.
public struct ChatMessage: Identifiable, Codable, Hashable, Sendable {
    public var seq: Int64
    public var role: ChatRole
    public var text: String
    public var at: Date
    /// Who wrote an assistant message: "Written by Claude Code · Opus".
    public var harness: Harness?
    public var model: String?
    public var card: ChatCard?
    public var tool: ChatToolStep?
    public var usage: ChatUsage?

    public var id: Int64 { seq }

    public init(
        seq: Int64, role: ChatRole, text: String, at: Date, harness: Harness? = nil, model: String? = nil,
        card: ChatCard? = nil, tool: ChatToolStep? = nil, usage: ChatUsage? = nil
    ) {
        self.seq = seq
        self.role = role
        self.text = text
        self.at = at
        self.harness = harness
        self.model = model
        self.card = card
        self.tool = tool
        self.usage = usage
    }
}

/// A chat with its messages (oldest first).
public struct Chat: Identifiable, Codable, Hashable, Sendable {
    public var id: String
    /// `owner/name` of the repo the chat reads, if any.
    public var repo: String?
    /// The optimization this chat steers, if any.
    public var optimization: String?
    public var harness: Harness
    public var model: String?
    public var state: ChatState
    public var messages: [ChatMessage]
    /// The reply being written, when a turn is in progress.
    public var pending: String?
    /// What `ChatClient.stream` continues from.
    public var lastEventID: Int64
    public var hasMore: Bool
    /// The first chat with a harness says what leaves the Mac.
    public var dataNotice: String?
    public var updatedAt: Date

    public init(
        id: String, repo: String? = nil, optimization: String? = nil, harness: Harness, model: String? = nil,
        state: ChatState = .idle, messages: [ChatMessage] = [], pending: String? = nil, lastEventID: Int64 = 0,
        hasMore: Bool = false, dataNotice: String? = nil, updatedAt: Date = Date(timeIntervalSince1970: 0)
    ) {
        self.id = id
        self.repo = repo
        self.optimization = optimization
        self.harness = harness
        self.model = model
        self.state = state
        self.messages = messages
        self.pending = pending
        self.lastEventID = lastEventID
        self.hasMore = hasMore
        self.dataNotice = dataNotice
        self.updatedAt = updatedAt
    }
}

/// Body of "start a chat". Everything is optional: an empty request is a Home chat on the default harness.
public struct NewChat: Hashable, Sendable {
    public var repo: String?
    public var optimization: String?
    public var harness: Harness?
    public var model: String?

    public init(repo: String? = nil, optimization: String? = nil, harness: Harness? = nil, model: String? = nil) {
        self.repo = repo
        self.optimization = optimization
        self.harness = harness
        self.model = model
    }
}

/// What the daemon answers to a sent message: its number, and which turn the reply belongs to.
public struct ChatReceipt: Hashable, Sendable {
    public var seq: Int64
    public var turn: Int

    public init(seq: Int64, turn: Int) {
        self.seq = seq
        self.turn = turn
    }
}

/// The first event of every stream connection.
public struct ChatReady: Hashable, Sendable {
    public var state: ChatState
    public var lastSeq: Int64
    public var lastEventID: Int64
    /// The events since `Last-Event-ID` are gone: fetch the chat again.
    public var resync: Bool
    /// The partial reply, when no replayed event carries it.
    public var pending: String?

    public init(state: ChatState, lastSeq: Int64, lastEventID: Int64, resync: Bool = false, pending: String? = nil) {
        self.state = state
        self.lastSeq = lastSeq
        self.lastEventID = lastEventID
        self.resync = resync
        self.pending = pending
    }
}

/// One event of a chat's stream.
public enum ChatEvent: Sendable, Equatable {
    case ready(ChatReady)
    /// A whole message: the user's (echoed) or a finished reply.
    case message(ChatMessage)
    /// Partial reply text; append. The finished `message` repeats it whole.
    case delta(String)
    /// A reasoning summary; append.
    case thinking(String)
    case tool(ChatToolStep)
    /// A draft card. `replaces` names the draft it supersedes.
    case card(ChatCard, replaces: String?)
    case note(ChatCard)
    case usage(ChatUsage)
    /// The turn is over.
    case done(turn: Int, outcome: ChatOutcome, usage: ChatUsage)
    /// The turn failed; `done` follows.
    case failed(ChatError)
}
