import RatchetKit
import SwiftUI

/// The system sidebar: the pages, then the recent runs, two levels at most and no action rows (New Optimization and search are
/// in the toolbar). The system paints it (glass, the scroll edge, the selection); nothing here has a background or an edge of its
/// own. A selected run wears a brass wash so it reads apart from a selected page.
struct SidebarView: View {
    @Environment(AppRouter.self) private var router
    @Environment(AppStore.self) private var store
    @Environment(\.appearsActive) private var appearsActive

    /// How many recent runs the sidebar lists.
    private static let recentLimit = 8

    /// The selected row: the open optimization (a run), else the page. No row while the New Optimization flow is open.
    private var selection: Binding<SidebarItem?> {
        Binding(
            get: {
                if router.creationChatID != nil { return .page(.home) }
                if router.page == .optimizations, let id = router.optimizationID { return .run(id) }
                return .page(router.page)
            },
            set: { item in
                switch item {
                case .page(let page)?: router.show(page)
                case .run(let id)?: router.open(optimization: id)
                case nil: break
                }
            })
    }

    private var recentRuns: [Optimization] {
        let sorted = store.optimizationStore.optimizations.sorted {
            ($0.startedAt ?? .distantPast) > ($1.startedAt ?? .distantPast)
        }
        return Array(sorted.prefix(Self.recentLimit))
    }

    var body: some View {
        List(selection: selection) {
            Section {
                ForEach(Page.sidebar) { page in
                    Label {
                        Text(page.title).kitType(.body)
                    } icon: {
                        KitIconView(icon: page.icon)
                    }
                    .tag(SidebarItem.page(page))
                    .help(page.shortcutNumber.map { "\(page.title) (Command-\($0))" } ?? page.title)
                    .accessibilityLabel(page.title)
                    .accessibilityHint(page.shortcutNumber.map { "Command-\($0)" } ?? "")
                }
            }
            Section {
                recentRunRows
            } header: {
                Text("Recent runs").kitType(.microStrong).foregroundStyle(Ink.textFaint)
            }
        }
        .listStyle(.sidebar)
        .accessibilityLabel("Sidebar")
    }

    @ViewBuilder
    private var recentRunRows: some View {
        if !store.isLoaded {
            ForEach(0..<5, id: \.self) { _ in
                HStack(spacing: Space.sm) {
                    SkeletonBlock(width: Dim.icon, height: Dim.icon)
                    VStack(alignment: .leading, spacing: Space.xs) {
                        SkeletonBlock(width: Space.section * 3, height: Space.md)
                        SkeletonBlock(width: Space.section * 2, height: Space.sm)
                    }
                }
                .accessibilityHidden(true)
            }
        } else if recentRuns.isEmpty {
            Text("No runs yet. Runs appear here.")
                .kitType(.caption).foregroundStyle(Ink.textFaint)
        } else {
            ForEach(recentRuns) { optimization in
                let isSelected = router.page == .optimizations && router.optimizationID == optimization.id
                RunRow(optimization: optimization, isSelected: isSelected)
                    .tag(SidebarItem.run(optimization.id))
                    .listRowBackground(isSelected ? RunSelectionBackground(isActive: appearsActive) : nil)
            }
        }
    }
}

/// One recent run: its status shape (never colour alone), its name and one line with the status word.
struct RunRow: View {
    let optimization: Optimization
    let isSelected: Bool

    var body: some View {
        HStack(alignment: .top, spacing: Space.sm) {
            StatusShape(tone: optimization.status.tone, size: Dim.icon)
                .frame(height: Dim.textLine)
            VStack(alignment: .leading, spacing: 0) {
                Text(optimization.title)
                    .kitType(isSelected ? .bodyStrong : .body)
                    .foregroundStyle(Ink.text)
                    .lineLimit(1)
                Text(statusLine)
                    .kitType(.caption)
                    .foregroundStyle(Ink.textFaint)
                    .lineLimit(1)
            }
        }
        .dimsWhenInactive()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(optimization.title), \(statusLine)")
        .accessibilityAddTraits(isSelected ? .isSelected : [])
        .help("\(optimization.title): \(statusLine)")
    }

    /// "Running · 1 h 12 m left", "Merged · −13%", "No win · within noise".
    var statusLine: String {
        var line = optimization.status.title
        if optimization.status == .running, let left = optimization.secondsLeft {
            line += " · " + Elapsed.hm(left) + " left"
        } else if let change = optimization.change {
            line += " · " + Percent.signed(change)
        } else if optimization.status == .noWin {
            line += " · within noise"
        }
        return line
    }
}

/// The selected run's row: a brass wash and a brass edge, so a run reads apart from a page. Dims with the window.
private struct RunSelectionBackground: View {
    let isActive: Bool

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: Dim.radiusRow, style: .continuous)
        shape
            .fill(Ink.brassWash)
            .overlay(shape.strokeBorder(Ink.brassBorder, lineWidth: Dim.hairline))
            .padding(.horizontal, Space.xs)
            .opacity(isActive ? 1 : Dim.inactiveOpacity)
    }
}
