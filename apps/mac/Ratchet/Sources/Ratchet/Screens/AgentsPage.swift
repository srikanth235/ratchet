import AppKit
import RatchetKit
import SwiftUI

/// Agents (screen 7): the agent CLIs found on this Mac with how they are paid for, their default model, which one chats use
/// ("default") and the order optimizations try them in; the API keys, the xAI one too. A status that is not OK has its one
/// fix button beside it: Sign In, Add Key…, Re-detect. All of it is `AgentsPageModel`; this view draws it.
struct AgentsPage: View {
    @Environment(AppStore.self) private var store
    @Environment(ChatHub.self) private var hub
    @Environment(\.daemonActionsEnabled) private var actionsEnabled
    @State private var sheet: ConnectorSheetRequest?

    private var agents: AgentsPageModel { hub.agents }

    var body: some View {
        KitPage {
            PageHeader("Agents", subtitle: "The agent CLIs Ratchet drives on this Mac, and the keys it can use.") {
                Button("Add Agent…") { sheet = ConnectorSheetRequest(kind: .agent) }
                    .buttonStyle(.kit(.quiet))
                    .disabled(!actionsEnabled)
            }
            .gridProbe("header", area: "agents")
            if let error = agents.error {
                HStack(spacing: Space.md) {
                    KitIconView(icon: .alert).foregroundStyle(Ink.bad)
                    Text(error).kitType(.meta).foregroundStyle(Ink.text)
                    Button("Try Again") { Task { await agents.load() } }.buttonStyle(.kit(.quiet, small: true))
                }
                .accessibilityElement(children: .contain)
            }
            OrderRow()
                .gridProbe("order", area: "agents")
            section(
                "On this Mac", note: "your own logins · free while the Mac is awake", rows: agents.loginRows, id: "logins",
                trailing: AnyView(
                    Button(agents.isDetecting ? "Detecting…" : "Re-detect") { Task { await agents.redetect() } }
                        .buttonStyle(.kit(.link))
                        .disabled(agents.isDetecting)
                        .help("Look for the agent CLIs on this Mac again")))
            section("API keys", note: "run on this Mac or unattended in the cloud · billed to the key", rows: agents.keyRows, id: "keys", trailing: nil)
        }
        .gridArea("agents", inset: Space.pageX)
        .sheet(item: $sheet, onDismiss: { Task { await agents.load(); hub.updateComposerOptions(store: store) } }) { AddConnectorSheet(request: $0) }
        .task {
            agents.onAddKey = { harness in sheet = ConnectorSheetRequest(kind: .agent, harness: harness) }
            await agents.load()
            hub.updateComposerOptions(store: store)
        }
    }

    private func section(_ title: String, note: String, rows: [AgentRow], id: String, trailing: AnyView?) -> some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            HStack(alignment: .firstTextBaseline, spacing: Space.md) {
                Text(title).kitType(.section).foregroundStyle(Ink.text).accessibilityAddTraits(.isHeader)
                Text(note).kitType(.meta).foregroundStyle(Ink.textFaint)
                if let trailing { trailing }
            }
            HairlineList {
                ForEach(rows) { row in AgentRowView(row: row) }
            }
        }
        .gridProbe(id, area: "agents")
    }
}

/// "Order · Claude Code → Codex → Grok · on this Mac" with Edit Order (the same popover as the draft card, without the Where
/// switch: a login runs on this Mac).
private struct OrderRow: View {
    @Environment(AppStore.self) private var store
    @Environment(ChatHub.self) private var hub
    @State private var open = false
    @State private var editor = AgentOrderEditor(order: [.claudeCode])

    var body: some View {
        HStack(spacing: Space.lg) {
            AgentBadge(text: "→")
            VStack(alignment: .leading, spacing: 0) {
                Text("Order").kitType(.bodyStrong).foregroundStyle(Ink.text)
                Text(hub.agents.orderSummary).kitType(.meta).foregroundStyle(Ink.textMuted)
                Text("If one is out of quota or signed out, the next one takes over.").kitType(.caption).foregroundStyle(Ink.textFaint)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Edit Order") {
                editor = hub.agents.orderEditor
                open = true
            }
            .buttonStyle(.kit(.quiet, small: true))
            .popover(isPresented: $open, arrowEdge: .bottom) {
                AgentOrderPopover(editor: $editor, showsWhere: false) {
                    hub.agents.saveOrder(editor)
                    hub.updateComposerOptions(store: store)
                    open = false
                }
            }
        }
        .padding(.vertical, Space.md)
        .overlay(alignment: .top) { Hairline() }
        .overlay(alignment: .bottom) { Hairline() }
        .accessibilityElement(children: .contain)
    }
}

/// Two letters in a rounded square: the harness's mark ("CC", "CX", "GK").
private struct AgentBadge: View {
    let text: String

    var body: some View {
        RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
            .strokeBorder(Ink.controlBorder, lineWidth: Dim.hairline)
            .frame(width: Dim.fieldHeight, height: Dim.fieldHeight)
            .overlay(Text(text).kitType(.microMono).foregroundStyle(Ink.textMuted))
            .accessibilityHidden(true)
    }
}

extension Harness {
    /// "CC", "CX", "GK".
    var initials: String {
        switch self {
        case .claudeCode: "CC"
        case .codex: "CX"
        case .grok: "GK"
        }
    }
}

extension AgentStatus {
    var tone: StatusTone {
        switch self {
        case .signedIn, .valid: .ok
        case .expired: .wait
        case .signedOut, .notInstalled, .noKey: .off
        }
    }
}

private struct AgentRowView: View {
    let row: AgentRow
    @Environment(AppStore.self) private var store
    @Environment(ChatHub.self) private var hub
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    private var model: String? {
        guard row.kind == .login else { return nil }
        let catalog = hub.agents.catalog
        return catalog.choice(for: row.harness).model.map { "default model " + $0.title }
    }

    /// "Included in your plan · plan limits apply · default model Opus".
    private var extraLine: String {
        [row.billingNote, model].compactMap { $0 }.joined(separator: " · ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            HairlineRowNoLine {
                AgentBadge(text: row.harness.initials)
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: Space.sm) {
                        Text(row.title).kitType(.bodyStrong).foregroundStyle(Ink.text).lineLimit(1)
                        if row.isChatDefault { DefaultTag() }
                    }
                    Text(row.detail).kitType(.meta).foregroundStyle(Ink.textFaint).lineLimit(2)
                    if !extraLine.isEmpty {
                        Text(extraLine).kitType(.caption).foregroundStyle(Ink.textFaint).lineLimit(2)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                StatusLabel.shaped(row.status.tone, row.status.word)
                    .frame(minWidth: Space.section * 4, alignment: .leading)
                if let fix = row.fix {
                    Button(fix.title) { Task { await hub.agents.perform(fix) } }
                        .buttonStyle(.kit(.quiet, small: true))
                        .disabled(!actionsEnabled && fix != .redetect)
                        .accessibilityLabel("\(fix.title), \(row.title)")
                }
                KitMoreMenu("More actions for \(row.title)") {
                    if row.kind == .login {
                        Button("Use for Chat") { hub.agents.setChatDefault(row.harness); hub.updateComposerOptions(store: store) }
                            .disabled(!row.status.isOK || row.isChatDefault)
                    }
                    Button("Re-detect") { Task { await hub.agents.redetect() } }
                }
            }
            if let help = hub.signInHelp, help.harness == row.harness, row.kind == .login, !row.status.isOK {
                SignInNote(help: help)
            }
        }
        .overlay(alignment: .bottom) { Hairline() }
        .accessibilityElement(children: .contain)
    }
}

/// The row without its closing hairline (the row view above draws it under the sign-in note too).
private struct HairlineRowNoLine<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        HStack(spacing: Space.lg) { content }
            .padding(.vertical, Dim.rowPadding)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct DefaultTag: View {
    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusXS, style: .continuous)
        Text("default").kitType(.micro).foregroundStyle(Ink.brassText)
            .padding(.horizontal, Space.sm)
            .padding(.vertical, Space.xs / 2)
            .overlay(shape.strokeBorder(Ink.brassBorder, lineWidth: Dim.hairline))
            .help("Chats start on this agent")
            .accessibilityLabel("default for chat")
    }
}

/// After Sign In: the command to run in Terminal (a login cannot be done from here), with Copy and Open Terminal. The page
/// re-detects when the person comes back to the app.
private struct SignInNote: View {
    let help: SignInHelp
    @Environment(ChatHub.self) private var hub

    var body: some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            CommandText(text: "Run `\(help.command)` in Terminal, then choose Re-detect.", type: .meta, color: Ink.textMuted)
            HStack(spacing: Space.sm) {
                Button("Copy Command") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(help.command, forType: .string)
                }
                .buttonStyle(.kit(.quiet, small: true))
                Button("Open Terminal") {
                    NSWorkspace.shared.open(URL(filePath: "/System/Applications/Utilities/Terminal.app"))
                }
                .buttonStyle(.kit(.quiet, small: true))
                Button("Re-detect") { Task { await hub.agents.redetect() } }
                    .buttonStyle(.kit(.link, small: true))
            }
        }
        .padding(.leading, Dim.fieldHeight + Space.lg)
        .padding(.bottom, Space.md)
    }
}
