import RatchetKit
import SwiftUI

// What a page shows when there is nothing to show yet, while it loads, and when the background service is down. Matches the
// v0.3.1 screens 11 (empty first launch), 12 (daemon down) and 14 (loading).

extension EnvironmentValues {
    /// False while the background service is not answering. Actions that need it (New Optimization, Stop, Run Now, Open Pull
    /// Request, Add Connector…, End Now) are disabled; reading what is already there still works.
    @Entry var daemonActionsEnabled: Bool = true
}

// MARK: - Daemon down

/// "Ratchet's background service isn't running. Data below may be out of date · last updated 14:02." On every page, above it,
/// with the one action that fixes it and a way to the details. Replaces the full-window status screen once there is data to show.
struct DaemonBanner: View {
    @Environment(AppStore.self) private var store
    @Environment(DaemonController.self) private var daemon
    @Environment(AppRouter.self) private var router
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
        HStack(alignment: .center, spacing: Space.md) {
            KitIconView(icon: .alert).foregroundStyle(Ink.bad)
            VStack(alignment: .leading, spacing: 0) {
                Text(headline).kitType(.bodyStrong).foregroundStyle(Ink.text)
                Text(detail).kitType(.meta).foregroundStyle(Ink.textMuted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            action
            Button("Details") {
                router.settingsTab = .thisMac
                openSettings()
            }
            .buttonStyle(.kit(.link, small: true))
        }
        .padding(.horizontal, Space.lg)
        .padding(.vertical, Space.sm)
        .background(shape.fill(Ink.badWash))
        .overlay(shape.strokeBorder(Ink.bad.opacity(0.5), lineWidth: Dim.hairline))
        .dimsWhenInactive()
        .accessibilityElement(children: .contain)
    }

    private var headline: String {
        switch daemon.phase {
        case .starting, .checking, .running: "Ratchet's background service is starting."
        case .needsApproval: "Ratchet's background service needs your OK."
        case .failed, .stopped: "Ratchet's background service isn't running."
        }
    }

    private var detail: String {
        var line = "What you see may be out of date"
        if let updated = store.lastUpdated { line += " · last updated " + Elapsed.timeOfDay(updated) }
        return line + ". Actions that need the service are off."
    }

    @ViewBuilder
    private var action: some View {
        switch daemon.phase {
        case .needsApproval:
            Button("Open Login Items") { daemon.openLoginItems() }.buttonStyle(.kit(.quiet, small: true))
        case .failed:
            if daemon.isUnbundled {
                Button("Check again") { daemon.probeNow() }.buttonStyle(.kit(.quiet, small: true))
            } else {
                Button("Try Again") { daemon.startAgain() }.buttonStyle(.kit(.quiet, small: true))
            }
        case .stopped:
            Button("Start Service") { daemon.startAgain() }.buttonStyle(.kit(.quiet, small: true))
        case .starting, .checking, .running:
            ProgressView().controlSize(.small)
        }
    }
}

// MARK: - Empty and loading

/// A page that has nothing to say yet: the mark, one sentence, one action.
struct PagePlaceholder: View {
    let page: Page
    let note: String

    var body: some View {
        KitPage {
            PageHeader(page.title)
            VStack(spacing: Space.lg) {
                RatchetMarkView(height: Dim.markLarge)
                Text(page.title).kitType(.pageTitle).foregroundStyle(Ink.text)
                Text(note)
                    .kitType(.body).foregroundStyle(Ink.textMuted)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: Layout.stateWidth)
            }
            .frame(maxWidth: .infinity)
            .padding(.top, Space.block)
            .accessibilityElement(children: .combine)
        }
        .gridArea("placeholder", inset: Space.pageX)
    }
}

/// "No wins yet. Start your first optimization." and a way to try the demo first.
struct EmptyOverview: View {
    /// What the daemon says for this phase ("No wins yet" / "Start your first optimization."); the same words when it says none.
    var text: EmptyStateText?
    @Environment(AppRouter.self) private var router
    @Environment(AppStore.self) private var store
    @Environment(DaemonController.self) private var daemon
    @Environment(\.daemonActionsEnabled) private var actionsEnabled
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        VStack(spacing: Space.lg) {
            RatchetMarkView(height: Dim.markHero)
            Text(text?.title ?? "No wins yet").kitType(.pageTitle).foregroundStyle(Ink.text)
            Text(text?.body ?? "Start your first optimization.").kitType(.body).foregroundStyle(Ink.textMuted)
            Button("New Optimization") { router.newOptimization() }
                .buttonStyle(.kit(.primary))
                .disabled(!actionsEnabled)
            if daemon.management == .managed && !store.isDemoData {
                HStack(spacing: Space.xs) {
                    Text("Or").kitType(.meta).foregroundStyle(Ink.textFaint)
                    Button("try a demo") {
                        router.settingsTab = .thisMac
                        openSettings()
                    }
                    .buttonStyle(.kit(.link))
                    .help("Demo mode shows made-up data and uses no accounts. Switch it on in Settings, This Mac.")
                    Text("first.").kitType(.meta).foregroundStyle(Ink.textFaint)
                }
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.top, Space.block * 2)
    }
}

/// A grey block that stands where content will be.
struct SkeletonBlock: View {
    var width: CGFloat?
    var height: CGFloat = Space.md

    var body: some View {
        RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
            .fill(Ink.track)
            .frame(width: width, height: height)
            .accessibilityHidden(true)
    }
}

/// The Overview while the first load is under way: the hero numbers, the chart, the side column and the rows as grey blocks.
struct OverviewSkeleton: View {
    var body: some View {
        VStack(alignment: .leading, spacing: Space.block) {
            HStack(alignment: .top, spacing: Space.section) {
                ForEach(0..<3, id: \.self) { _ in
                    VStack(alignment: .leading, spacing: Space.sm) {
                        SkeletonBlock(width: Space.section * 3, height: Space.md)
                        SkeletonBlock(width: Space.section * 5, height: Space.section + Space.md)
                        SkeletonBlock(height: Space.xs)
                        SkeletonBlock(width: Space.section * 4, height: Space.md)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            HStack(alignment: .top, spacing: Space.pageX) {
                SkeletonBlock(height: Dim.chartHeight + Space.section)
                VStack(spacing: Space.lg) {
                    SkeletonBlock(height: Space.section * 3)
                    SkeletonBlock(height: Space.section * 2)
                }
                .frame(width: Layout.sideColumnNarrow - Space.section * 2)
            }
            VStack(spacing: Space.md) {
                ForEach(0..<4, id: \.self) { _ in SkeletonBlock(height: Space.md) }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Loading your optimizations")
    }
}
