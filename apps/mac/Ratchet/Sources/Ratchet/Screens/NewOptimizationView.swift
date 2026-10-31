import RatchetKit
import SwiftUI

/// New optimization: Goal & repo, Connections, Environment, Budget & time. Fills the window like the design, with
/// Back and Continue pinned at the bottom.
struct NewOptimizationView: View {
    @Bindable var flow: NewOptimizationFlow
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: Space.block) {
                    header
                        .gridProbe("header", area: "wizard")
                    stepContent
                        .gridProbe("step", area: "wizard")
                }
                .frame(maxWidth: 1080, alignment: .leading)
                .padding(.horizontal, Space.pageX)
                .padding(.top, Space.pageTop)
                .padding(.bottom, Space.lg)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            footer
        }
        .background(Ink.canvas)
        .gridArea("wizard")
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: Space.md) {
            StepProgress(current: flow.step.rawValue, total: WizardStep.allCases.count, caption: flow.step.caption)
            VStack(alignment: .leading, spacing: Space.sm) {
                Text(flow.title).kitType(.pageTitle).foregroundStyle(Ink.text).lineLimit(2)
                Text(subtitle).kitType(.body).foregroundStyle(Ink.textFaint)
            }
        }
    }

    private var subtitle: String {
        if flow.step == .environment, let draft = flow.draft {
            return "Drafted by \(draft.draftedBy) from the repo and your goal · "
                + Elapsed.minutes(draft.draftSeconds / 60) + " · " + Money.exact(draft.draftCostUSD)
        }
        return flow.subtitle
    }

    @ViewBuilder
    private var stepContent: some View {
        switch flow.step {
        case .goal: GoalStep(flow: flow)
        case .connections: ConnectionsStep(flow: flow)
        case .environment: EnvironmentStep(flow: flow)
        case .budget: BudgetStep(flow: flow)
        }
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: Space.md) {
            Button(flow.step == .goal ? "Cancel" : "Back") {
                if flow.step == .goal { router.closeWizard() } else { Task { await flow.back() } }
            }
            .buttonStyle(.kit(.quiet))
            .keyboardShortcut(.cancelAction)
            Spacer()
            if let error = flow.error {
                Text(error).kitType(.meta).foregroundStyle(Ink.bad)
            } else {
                Text(note).kitType(.meta).foregroundStyle(Ink.textFaint)
            }
            primaryButton
        }
        .padding(.horizontal, Space.pageX)
        .padding(.vertical, Space.lg)
        .frame(maxWidth: .infinity)
        .overlay(alignment: .top) { Hairline() }
        .background(Ink.canvas)
    }

    private var note: String {
        switch flow.step {
        case .goal: ""
        case .connections: "Remembered for this repo"
        case .environment: "Locked once approved. Agents change code only."
        case .budget: "Whichever limit comes first ends the run."
        }
    }

    @ViewBuilder
    private var primaryButton: some View {
        switch flow.step {
        case .environment where flow.draft?.state != .approved:
            Button("Approve") { Task { await flow.approveDraft() } }
                .buttonStyle(.kit(.primary))
                .disabled(flow.draft == nil || flow.isBusy)
                .keyboardShortcut(.defaultAction)
        case .budget:
            Button("Start optimization") { start() }
                .buttonStyle(.kit(.primary))
                .disabled(!flow.canContinue || flow.isBusy)
                .keyboardShortcut(.defaultAction)
        default:
            Button("Continue") { Task { await flow.next() } }
                .buttonStyle(.kit(.primary))
                .disabled(!flow.canContinue || flow.isBusy)
                .keyboardShortcut(.defaultAction)
        }
    }

    private func start() {
        Task {
            guard let started = await flow.start() else { return }
            await store.register(started)
            router.open(optimization: started.id)
        }
    }
}
