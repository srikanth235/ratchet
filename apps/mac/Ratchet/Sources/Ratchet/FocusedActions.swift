import AppKit
import RatchetKit
import SwiftUI

// Commands reach the window through focused values: the Optimizations table and the detail page publish what is selected, the
// menu bar acts on it. A menu item with nothing selected stays visible and is disabled.

struct OptimizationSelectionKey: FocusedValueKey {
    typealias Value = [String]
}

struct FindActionKey: FocusedValueKey {
    typealias Value = FindAction
}

/// Moves the keyboard to the page's search field.
struct FindAction {
    let run: () -> Void
}

extension FocusedValues {
    /// The ids of the optimizations selected in the focused window (the table's selection, or the one that is open).
    var optimizationSelection: [String]? {
        get { self[OptimizationSelectionKey.self] }
        set { self[OptimizationSelectionKey.self] = newValue }
    }

    /// The focused page's search field, when it has one.
    var findAction: FindAction? {
        get { self[FindActionKey.self] }
        set { self[FindActionKey.self] = newValue }
    }
}

/// The Optimization menu (with Add Connector… and Export Report…) and Find in Edit.
struct OptimizationCommands: Commands {
    let store: AppStore
    let router: AppRouter
    let daemon: DaemonController
    let connectors: ConnectorsPageState
    @FocusedValue(\.optimizationSelection) private var selection
    @FocusedValue(\.findAction) private var find

    private var selected: Optimization? {
        guard let selection, selection.count == 1 else { return nil }
        return store.optimizationStore.optimization(id: selection[0])
    }

    private var serviceUp: Bool { daemon.phase.isRunning }

    var body: some Commands {
        CommandMenu("Optimization") {
            Button("Open") {
                if let selected { router.open(optimization: selected.id) }
            }
            .keyboardShortcut("o")
            .disabled(selected == nil)
            Divider()
            Button("Run Now") {
                if let selected { Task { await store.optimizationStore.runNow(selected.id) } }
            }
            .keyboardShortcut("r", modifiers: [.command, .shift])
            .disabled(selected.map { !OptimizationAction.canRun($0) || !serviceUp } ?? true)
            Button("Stop…") {
                if let selected { router.pendingStop = selected.id }
            }
            .keyboardShortcut(".")
            .disabled(selected.map { !OptimizationAction.canStop($0) || !serviceUp } ?? true)
            Button("Open Pull Request") {
                if let selected { Task { await OptimizationAction.openPullRequest(selected, store: store) } }
            }
            .keyboardShortcut("p", modifiers: [.command, .shift])
            .disabled(selected.map { !OptimizationAction.canOpenPullRequest($0) || (!serviceUp && $0.prNumber == nil) } ?? true)
            Divider()
            Button("Copy Link") {
                if let selected { OptimizationAction.copyLinks([selected]) }
            }
            .disabled(selected == nil)
            Divider()
            Button("Add Connector…") {
                router.show(.connectors)
                connectors.sheet = ConnectorSheetRequest(kind: .github)
            }
            .keyboardShortcut("n", modifiers: [.command, .shift])
            .disabled(!serviceUp)
            Button("Export Report…") {
                if let selected { Task { await OptimizationAction.exportReport(selected, store: store) } }
            }
            .keyboardShortcut("e", modifiers: [.command, .shift])
            .disabled(selected == nil)
        }
        CommandGroup(after: .textEditing) {
            Button("Find…") { find?.run() }
                .keyboardShortcut("f")
                .disabled(find == nil)
        }
    }
}
