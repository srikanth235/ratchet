import Foundation
import Observation

/// What the Agents page remembers about the person's choices: which harness chats use by default, and the order optimizations
/// try them in. Kept by the app (the daemon has no setting for either); a chat or a new optimization starts from them.
@MainActor
public protocol HarnessPreferences: AnyObject {
    var chatDefault: Harness? { get set }
    var optimizationOrder: [Harness] { get set }
}

@MainActor
public final class MemoryHarnessPreferences: HarnessPreferences {
    public var chatDefault: Harness?
    public var optimizationOrder: [Harness]

    public init(chatDefault: Harness? = nil, optimizationOrder: [Harness] = []) {
        self.chatDefault = chatDefault
        self.optimizationOrder = optimizationOrder
    }
}

@MainActor
public final class UserDefaultsHarnessPreferences: HarnessPreferences {
    private let defaults: UserDefaults
    private static let chatKey = "harness.chatDefault"
    private static let orderKey = "harness.optimizationOrder"

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public var chatDefault: Harness? {
        get { defaults.string(forKey: Self.chatKey).flatMap { Harness(rawValue: $0) } }
        set { defaults.set(newValue?.rawValue, forKey: Self.chatKey) }
    }

    public var optimizationOrder: [Harness] {
        get { (defaults.stringArray(forKey: Self.orderKey) ?? []).compactMap { Harness(rawValue: $0) } }
        set { defaults.set(newValue.map(\.rawValue), forKey: Self.orderKey) }
    }
}

/// What a broken status offers next to it: every non-OK status has one button.
public enum AgentFix: Equatable, Sendable {
    case signIn(Harness)
    case addKey(Harness)
    case redetect

    public var title: String {
        switch self {
        case .signIn: "Sign In"
        case .addKey: "Add Key…"
        case .redetect: "Re-detect"
        }
    }
}

public enum AgentStatus: Equatable, Sendable {
    case signedIn, signedOut, notInstalled
    /// An API key that works.
    case valid
    case expired
    case noKey

    /// The word beside the shape.
    public var word: String {
        switch self {
        case .signedIn: "Signed in"
        case .signedOut: "Signed out"
        case .notInstalled: "Not installed"
        case .valid: "Valid"
        case .expired: "Expired"
        case .noKey: "No key"
        }
    }

    public var isOK: Bool { self == .signedIn || self == .valid }
}

public enum AgentRowKind: Equatable, Sendable {
    /// The user's own login on this Mac: free while the Mac is awake, limited by the plan.
    case login
    /// An API key: runs on this Mac or unattended in the cloud, billed to the key.
    case apiKey
}

/// One row of the Agents page.
public struct AgentRow: Identifiable, Equatable, Sendable {
    public var id: String
    public var harness: Harness
    public var kind: AgentRowKind
    /// "Claude Code · your login", "Codex · ci key".
    public var title: String
    /// "~/.local/bin/claude · v2.1.314 · 6 optimizations use it".
    public var detail: String
    public var status: AgentStatus
    public var fix: AgentFix?
    /// The harness chats start on, shown as "default".
    public var isChatDefault: Bool
    /// What the harness costs: "Included in your plan" or "Billed to the key".
    public var billingNote: String?
}

/// The Agents page: harnesses detected on this Mac, their keys, billing, the default for chat and the order optimizations try
/// them in, and a fix button next to every status that is not OK (Sign In, Add Key…, Re-detect).
@MainActor @Observable
public final class AgentsPageModel {
    public private(set) var catalog: HarnessCatalog = .empty
    public private(set) var connectors: [Connector] = []
    public private(set) var isLoading = false
    public private(set) var isDetecting = false
    public private(set) var error: String?
    /// How many optimizations list each harness, from the optimizations (set by the app).
    public var usage: [Harness: Int] = [:]
    /// Rows the page says to put in front of the person: the app shows its sign-in help (`codex login` in Terminal) or its Add
    /// Key sheet; set by the app. The model re-detects after the person comes back.
    @ObservationIgnored public var onSignIn: (Harness, String) -> Void = { _, _ in }
    @ObservationIgnored public var onAddKey: (Harness) -> Void = { _ in }

    @ObservationIgnored private let client: any ConnectorsClient
    @ObservationIgnored private let preferences: any HarnessPreferences

    public init(client: any ConnectorsClient, preferences: any HarnessPreferences) {
        self.client = client
        self.preferences = preferences
    }

    // MARK: Loading

    public func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            async let found = client.detectedHarnesses(redetect: false)
            async let list = client.connectors()
            let (detected, all) = try await (found, list)
            catalog = HarnessCatalog(detected)
            connectors = all
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// "Re-detect": probes the machine again.
    public func redetect() async {
        isDetecting = true
        defer { isDetecting = false }
        do {
            catalog = HarnessCatalog(try await client.detectedHarnesses(redetect: true))
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }

    // MARK: Choices

    /// The harness a chat starts on: the person's choice while it is ready, else the first ready one.
    public var chatDefault: Harness? {
        if let chosen = preferences.chatDefault, catalog.isReady(chosen) { return chosen }
        return catalog.ready.first
    }

    /// Only a harness that is ready can be the default for chat.
    public func setChatDefault(_ harness: Harness) {
        guard catalog.isReady(harness) else { return }
        preferences.chatDefault = harness
    }

    /// The order optimizations try harnesses in, the person's list first and ready harnesses they left out after it; harnesses
    /// that are not ready are not in it.
    public var optimizationOrder: [Harness] {
        let ready = catalog.ready
        var order = preferences.optimizationOrder.filter { ready.contains($0) }
        for harness in ready where !order.contains(harness) { order.append(harness) }
        return order
    }

    /// The editor behind "Edit Order": the list, and where they run (this Mac: a login cannot run in the cloud).
    public var orderEditor: AgentOrderEditor {
        AgentOrderEditor(order: optimizationOrder, placement: .local, available: catalog.ready)
    }

    public func saveOrder(_ editor: AgentOrderEditor) {
        preferences.optimizationOrder = editor.order
    }

    /// "Claude Code → Codex → Grok · on this Mac": the Order row.
    public var orderSummary: String { orderEditor.summary }

    // MARK: Rows

    private func keyConnector(for harness: Harness) -> Connector? {
        connectors.first { $0.kind == .agent && $0.origin == .apiKey && $0.harness == harness }
    }

    /// "On this Mac": one row per harness, your own login.
    public var loginRows: [AgentRow] {
        let defaultHarness = chatDefault
        return Harness.allCases.map { harness in
            let found = catalog.detection(of: harness)
            let status: AgentStatus
            var fix: AgentFix?
            if found?.installed != true {
                status = .notInstalled
                fix = .redetect
            } else if found?.auth == .signedOut {
                status = .signedOut
                fix = .signIn(harness)
            } else {
                status = .signedIn
            }
            var pieces: [String] = []
            if let path = found?.path { pieces.append(path) }
            if let version = found?.version { pieces.append("v" + version) }
            if status == .notInstalled { pieces.append(found?.hint ?? "Not found on this Mac") }
            if let count = usage[harness], count > 0 { pieces.append("\(Plural.count(count, "optimization")) use\(count == 1 ? "s" : "") it") }
            let billing: String?
            switch found?.billing {
            case .subscription?: billing = "Included in your plan · plan limits apply"
            case .apiKey?: billing = "Billed to the API key"
            case nil: billing = nil
            }
            return AgentRow(
                id: "login-" + harness.rawValue, harness: harness, kind: .login, title: harness.title + " · your login",
                detail: pieces.joined(separator: " · "), status: status, fix: fix, isChatDefault: harness == defaultHarness && status.isOK,
                billingNote: billing)
        }
    }

    /// "API keys": the keys that were added, then a "No key" row for each harness without one.
    public var keyRows: [AgentRow] {
        var rows: [AgentRow] = []
        for harness in Harness.allCases {
            let keys = connectors.filter { $0.kind == .agent && $0.origin == .apiKey && $0.harness == harness }
            for key in keys {
                let status: AgentStatus = key.state.isUsable ? .valid : .expired
                rows.append(AgentRow(
                    id: "key-" + key.id, harness: harness, kind: .apiKey, title: key.name, detail: key.detail, status: status,
                    fix: status == .valid ? nil : .addKey(harness), isChatDefault: false, billingNote: "Billed to the key"))
            }
            if keys.isEmpty {
                rows.append(AgentRow(
                    id: "nokey-" + harness.rawValue, harness: harness, kind: .apiKey, title: harness.title + " · " + harness.vendor + " key",
                    detail: harness.vendor + " key · none yet", status: .noKey, fix: .addKey(harness), isChatDefault: false, billingNote: nil))
            }
        }
        return rows
    }

    /// Every row that is not OK has exactly one fix.
    public var rowsNeedingAFix: [AgentRow] { (loginRows + keyRows).filter { !$0.status.isOK && $0.fix != nil } }

    // MARK: Fixes

    /// Runs the button next to a status. Sign In: checks the login again and, when it is still signed out, hands the app the
    /// command to run (`codex login`); the page re-detects when the person comes back.
    public func perform(_ fix: AgentFix) async {
        switch fix {
        case .redetect:
            await redetect()
        case .addKey(let harness):
            onAddKey(harness)
        case .signIn(let harness):
            if let login = connectors.first(where: { $0.kind == .agent && $0.origin == .login && $0.harness == harness }) {
                _ = try? await client.signInConnector(id: login.id)
            }
            await redetect()
            if !catalog.isReady(harness) { onSignIn(harness, Self.signInCommand(harness)) }
        }
    }

    /// What to run in Terminal to sign in.
    public static func signInCommand(_ harness: Harness) -> String {
        switch harness {
        case .claudeCode: "claude auth login"
        case .codex: "codex login"
        case .grok: "grok login"
        }
    }
}
