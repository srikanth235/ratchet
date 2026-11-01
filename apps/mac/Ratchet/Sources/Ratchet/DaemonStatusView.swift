import AppKit
import RatchetKit
import SwiftUI

/// What the window shows while `ratchetd` is not answering: starting, waiting for the user's OK in Login Items, failed,
/// or stopped. Quiet by design: one sentence and, when the user can do something, one button.
struct DaemonStatusView: View {
    @Environment(DaemonController.self) private var daemon

    var body: some View {
        VStack(spacing: Space.xxl) {
            RatchetMarkView(height: 36)
            VStack(spacing: Space.sm) {
                Text(title).kitType(.pageTitle).foregroundStyle(Ink.text)
                Text(message)
                    .kitType(.body).foregroundStyle(Ink.textMuted)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            }
            actions
        }
        .padding(Space.pageX)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Ink.canvas)
        .accessibilityElement(children: .contain)
    }

    private var title: String {
        switch daemon.phase {
        case .needsApproval: "Allow Ratchet to run in the background"
        case .failed: "Ratchet could not start"
        case .stopped: "The background service is off"
        case .checking, .starting, .running: "Starting Ratchet…"
        }
    }

    private var message: String {
        switch daemon.phase {
        case .needsApproval:
            "Optimizations keep running after you quit the app. macOS asks for your OK once: switch on Ratchet under Login Items in System Settings."
        case .failed(let why): why
        case .stopped: "Optimizations cannot run while it is off."
        case .checking, .starting, .running: "Getting the background service ready. This takes a few seconds."
        }
    }

    @ViewBuilder
    private var actions: some View {
        switch daemon.phase {
        case .needsApproval:
            HStack(spacing: Space.sm) {
                Button("Open Login Items") { daemon.openLoginItems() }.buttonStyle(.kit(.primary))
                Button("Check again") { daemon.recheck() }.buttonStyle(.kit(.quiet))
            }
        case .failed:
            HStack(spacing: Space.sm) {
                Button("Try again") { daemon.startAgain() }.buttonStyle(.kit(.primary))
                Button("Open log") { DaemonLog.reveal() }.buttonStyle(.kit(.quiet))
            }
        case .stopped:
            Button("Start") { daemon.startAgain() }.buttonStyle(.kit(.primary))
        case .checking, .starting, .running:
            ProgressView().controlSize(.small)
        }
    }
}

enum DaemonLog {
    static func reveal() {
        let url = URL.applicationSupportDirectory.appending(path: "Ratchet/ratchetd.log")
        if FileManager.default.fileExists(atPath: url.path) {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } else {
            NSWorkspace.shared.open(url.deletingLastPathComponent())
        }
    }
}
