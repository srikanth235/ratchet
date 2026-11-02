import RatchetKit
import SwiftUI

/// Home: the value story, then the composer. Sending opens the creation conversation in this place (`CreationView`), so the
/// sidebar keeps Home selected while the setup agent drafts. The composer is glass inside one `KitGlassContainer` (`KitComposer`).
struct HomePage: View {
    @Environment(AppRouter.self) private var router

    var body: some View {
        if let chatID = router.creationChatID {
            CreationView(chatID: chatID)
        } else {
            HomeContent()
        }
    }
}

private struct HomeContent: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router
    @Environment(ChatHub.self) private var hub
    @Environment(\.daemonActionsEnabled) private var actionsEnabled

    private static let suggestions = [
        "Cut CLI cold start", "Shrink the web bundle", "Improve forecast accuracy", "Try a demo",
    ]

    var body: some View {
        @Bindable var composer = hub.composer
        KitPage {
            HomeValueHeader(summary: hub.story.home)
                .gridProbe("value", area: "home")
            VStack(spacing: Space.sm) {
                Text("What should get better?").kitType(.pageTitle).foregroundStyle(Ink.text)
                    .accessibilityAddTraits(.isHeader)
                Text("Describe it once. Ratchet runs it and opens a pull request with what it proves.")
                    .kitType(.body).foregroundStyle(Ink.textMuted)
                    .multilineTextAlignment(.center)
            }
            .frame(maxWidth: .infinity)
            .padding(.top, Space.xxl)
            .gridProbe("title", area: "home")
            VStack(spacing: Space.md) {
                KitComposer(
                    text: $composer.text, placeholder: "Make checkout p95 faster in shop-api. Run it every night.",
                    isSending: false, canSend: composer.canSend && actionsEnabled, onSend: send
                ) {
                    HomeChips(composer: composer)
                }
                .frame(maxWidth: Layout.composerWidth)
                .gridProbe("composer", area: "home")
                composerNote(composer)
                    .frame(maxWidth: Layout.composerWidth, alignment: .leading)
                suggestionRow(composer)
                    .frame(maxWidth: Layout.composerWidth)
                    .padding(.top, Space.sm)
                if !hub.homeNoticeClosed, let first = composer.agents.order.first, composer.options.catalog.isReady(first) {
                    NoticeLine(text: Self.notice(first), closeLabel: "Dismiss the first-use notice") { hub.closeHomeNotice() }
                        .frame(maxWidth: Layout.composerWidth)
                        .padding(.top, Space.sm)
                        .gridProbe("notice", area: "home")
                }
            }
            .frame(maxWidth: .infinity)
        }
        .gridArea("home", inset: Space.pageX)
        .task { await hub.refresh(store: store) }
        .onChange(of: store.repos.count) { _, _ in hub.updateComposerOptions(store: store) }
        .onChange(of: store.connectorStore.connectors.count) { _, _ in hub.updateComposerOptions(store: store) }
    }

    /// "First time: your messages, the repo name and the files Claude Code reads go to Anthropic. Nothing else."
    static func notice(_ harness: Harness) -> String {
        "First time: your messages, the repo name and the files \(harness.title) reads go to \(harness.vendor). Ratchet sends nothing else."
    }

    private func send() {
        Task { await hub.sendFromHome(store: store, router: router) }
    }

    /// Why Send is off, or what failed to start; the words stay in the box either way.
    @ViewBuilder
    private func composerNote(_ composer: ComposerModel) -> some View {
        if !composer.options.catalog.detected.isEmpty && composer.options.catalog.ready.isEmpty {
            HStack(spacing: Space.md) {
                KitIconView(icon: .alert).foregroundStyle(Ink.bad)
                Text("No agent is signed in. Sign in to Claude Code, Codex or Grok first.")
                    .kitType(.meta).foregroundStyle(Ink.text)
                Button("Open Agents") { router.show(.agents) }
                    .buttonStyle(.kit(.quiet, small: true))
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Space.sm)
            .accessibilityElement(children: .contain)
        } else if let failure = composer.failure {
            HStack(spacing: Space.md) {
                KitIconView(icon: .alert).foregroundStyle(Ink.bad)
                Text(failure).kitType(.meta).foregroundStyle(Ink.text)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Space.sm)
            .accessibilityElement(children: .combine)
        } else {
            Text(ComposerModel.hint).kitType(.caption).foregroundStyle(Ink.textFaint)
                .padding(.horizontal, Space.sm)
        }
    }

    private func suggestionRow(_ composer: ComposerModel) -> some View {
        FlowLayout(spacing: Space.sm) {
            ForEach(Self.suggestions, id: \.self) { suggestion in
                Button(suggestion) { composer.text = suggestion == "Try a demo" ? "Try a demo optimization on the sample repo." : suggestion + "." }
                    .buttonStyle(.kit(.quiet, small: true))
                    .help("Put this in the box")
            }
        }
        .frame(maxWidth: .infinity, alignment: .center)
    }
}

/// The five chips under the box: repo, cloud, agents, limits, schedule. Each opens a popover that is titled like its label
/// ("Change agents").
private struct HomeChips: View {
    @Bindable var composer: ComposerModel

    var body: some View {
        FlowLayout(spacing: Space.sm) {
            PopoverChip(title: composer.title(of: .repo), label: ComposerChip.repo.popoverTitle, isSet: composer.isSet(.repo)) { close in
                ChoicePopover(
                    title: ComposerChip.repo.popoverTitle, options: composer.options.repos, selected: composer.repo, noneTitle: nil,
                    searchPrompt: "Search repositories", onSelect: { composer.repo = $0; close() }, done: close)
            }
            PopoverChip(title: composer.title(of: .cloud), label: ComposerChip.cloud.popoverTitle, isSet: composer.isSet(.cloud)) { close in
                ChoicePopover(
                    title: ComposerChip.cloud.popoverTitle, options: composer.options.cloudAccounts, selected: composer.cloudAccount,
                    noneTitle: "No cloud", onSelect: { composer.cloudAccount = $0; close() }, done: close)
            }
            PopoverChip(title: composer.title(of: .agents), label: ComposerChip.agents.popoverTitle, isSet: composer.isSet(.agents)) { close in
                AgentOrderPopover(editor: $composer.agents, done: close)
            }
            PopoverChip(title: composer.title(of: .limits), label: ComposerChip.limits.popoverTitle, isSet: composer.isSet(.limits)) { close in
                LimitsPopover(
                    editor: Binding(get: { composer.limits ?? LimitsEditor() }, set: { composer.limits = $0 }), done: close)
            }
            PopoverChip(title: composer.title(of: .schedule), label: ComposerChip.schedule.popoverTitle, isSet: composer.isSet(.schedule)) { close in
                SchedulePopover(kind: $composer.schedule, allowsNone: true, done: close)
            }
        }
    }
}

/// "Spent · 90 days $184" and "Value per year $12.5k delivered + $2.9k pending · 68×": the same numbers, in the same words,
/// as the Overview (both read `ValueStory`). Nothing at all before the first optimization.
struct HomeValueHeader: View {
    let summary: HomeSummary?
    @Environment(AppRouter.self) private var router

    var body: some View {
        if let summary, !summary.isEmpty {
            VStack(alignment: .leading, spacing: Space.md) {
                HStack(alignment: .firstTextBaseline, spacing: Space.section) {
                    VStack(alignment: .leading, spacing: Space.xs) {
                        Text(summary.spentCaption).kitType(.meta).foregroundStyle(Ink.textFaint)
                        Text(summary.story.spentText).kitType(.quote).foregroundStyle(Ink.text)
                    }
                    .accessibilityElement(children: .combine)
                    VStack(alignment: .leading, spacing: Space.xs) {
                        Text("Value per year").kitType(.meta).foregroundStyle(Ink.textFaint)
                        HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
                            Text(summary.story.deliveredText).kitType(.quote).foregroundStyle(Ink.text)
                            Text(Self.rest(of: summary.story)).kitType(.body).foregroundStyle(Ink.textMuted)
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("Value per year: " + summary.story.headline)
                }
                HStack(spacing: Space.sm) {
                    Text(summary.countsLine).kitType(.meta).foregroundStyle(Ink.textFaint)
                    if let failed = summary.failed {
                        Text("·").kitType(.meta).foregroundStyle(Ink.textFaint).accessibilityHidden(true)
                        KitIconView(icon: .alert, size: Dim.icon - Space.xs).foregroundStyle(Ink.bad)
                        Text("1 run failed").kitType(.meta).foregroundStyle(Ink.text)
                        Button(failed.title) { router.open(optimization: failed.optimizationID) }
                            .buttonStyle(.kit(.link))
                            .help(failed.detail)
                    }
                }
            }
        }
    }

    /// "delivered + $2.9k pending · 68×": what follows the big number.
    static func rest(of story: ValueStory) -> String {
        var text = "delivered"
        if let pending = story.pendingText { text += " + " + pending }
        if let multiple = story.returnText { text += " · " + multiple }
        return text
    }
}
