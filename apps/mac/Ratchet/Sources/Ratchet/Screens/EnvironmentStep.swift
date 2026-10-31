import RatchetKit
import SwiftUI

// MARK: - Step 3: environment

struct EnvironmentStep: View {
    @Bindable var flow: NewOptimizationFlow

    var body: some View {
        if let draft = flow.draft {
            VStack(alignment: .leading, spacing: Space.block) {
                quote(draft)
                HStack(alignment: .top, spacing: Space.pageX) {
                    resources(draft).frame(maxWidth: .infinity, alignment: .leading)
                    cost(draft).frame(width: 320, alignment: .leading)
                }
                requestChange
            }
        } else {
            HStack(spacing: Space.sm) {
                ProgressView().controlSize(.small)
                Text("Drafting the environment from the repo and your goal…").kitType(.body).foregroundStyle(Ink.textMuted)
            }
        }
    }

    // MARK: Headline

    private func quote(_ draft: EnvironmentDraft) -> some View {
        VStack(alignment: .leading, spacing: Space.sm + 2) {
            Text(headline(draft))
                .kitType(.quote)
                .foregroundStyle(Ink.text)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: Space.xs) {
                Text(draft.reason).kitType(.meta).foregroundStyle(Ink.textFaint)
                if draft.scope == .full {
                    Button("Use a smaller setup") { Task { await flow.sendRevision("Use a smaller setup") } }
                        .buttonStyle(.kit(.link))
                }
            }
        }
    }

    private func headline(_ draft: EnvironmentDraft) -> AttributedString {
        var text = AttributedString(draft.headline)
        if let range = text.range(of: draft.emphasis) {
            text[range].foregroundColor = Ink.brassText
        }
        return text
    }

    // MARK: What gets created

    private func resources(_ draft: EnvironmentDraft) -> some View {
        KitSection("What gets created") {
            VStack(alignment: .leading, spacing: Space.md) {
                HairlineList {
                    ForEach(draft.resources) { resource in
                        HairlineRow {
                            TitleStack(title: resource.name, subtitle: resource.detail)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            Text(resource.machine).kitType(.meta).foregroundStyle(Ink.textMuted)
                                .frame(width: 128, alignment: .leading)
                            Text(Money.perHour(resource.usdPerHour)).kitType(.mono).foregroundStyle(Ink.text)
                                .frame(width: 80, alignment: .trailing)
                        }
                    }
                }
                HStack(spacing: Space.xs) {
                    Text("Files").foregroundStyle(Ink.brassText)
                    Text("·").foregroundStyle(Ink.textFaint)
                    Text(draft.files.joined(separator: ", ")).foregroundStyle(Ink.textFaint)
                }
                .kitType(.meta)
            }
        }
    }

    // MARK: Cost

    private func cost(_ draft: EnvironmentDraft) -> some View {
        KitSection("Cost") {
            VStack(alignment: .leading, spacing: Space.lg) {
                HeroNumber(
                    label: "", value: Money.exact(draft.usdPerHour), unit: "/ h",
                    note: "ready in ~" + Elapsed.minutes(draft.readyMinutes) + " · about " + Money.exact(draft.cost(forHours: flow.timeLimitHours))
                        + " for a " + String(format: "%g", flow.timeLimitHours) + " h run")
                VStack(alignment: .leading, spacing: Space.xs) {
                    HStack(spacing: Space.sm) {
                        KitIconView(icon: .check, size: Dim.icon, weight: 1.6).foregroundStyle(Ink.ok)
                        Text("Passed Ratchet's checks").kitType(.body).foregroundStyle(Ink.textMuted)
                    }
                    Text(draft.checks).kitType(.meta).foregroundStyle(Ink.textFaint)
                        .padding(.leading, Space.xxl)
                }
            }
        }
    }

    // MARK: Ask for a change

    private var requestChange: some View {
        HStack(spacing: Space.md) {
            TextField("Ask for a change, e.g. “use RDS instead of a Postgres container”", text: $flow.revisionText)
                .kitField()
                .onSubmit { Task { await flow.sendRevision() } }
                .accessibilityLabel("Ask for a change")
            Button("Send") { Task { await flow.sendRevision() } }
                .buttonStyle(.kit(.quiet))
                .disabled(flow.revisionText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || flow.isBusy)
        }
    }
}

// MARK: - Step 4: budget and time

struct BudgetStep: View {
    @Bindable var flow: NewOptimizationFlow

    var body: some View {
        VStack(alignment: .leading, spacing: Space.block) {
            HStack(alignment: .top, spacing: Space.pageX) {
                limit(
                    title: "Budget", value: Money.short(flow.budgetUSD),
                    note: "Spending stops here. Agent keys and the sandbox both count.") {
                    Stepper(value: $flow.budgetUSD, in: 10...500, step: 10) { Text("Budget") }
                }
                limit(
                    title: "Time", value: String(format: "%g", flow.timeLimitHours), unit: "h",
                    note: "The sandbox switches off by itself when time is up.") {
                    Stepper(value: $flow.timeLimitHours, in: 0.5...24, step: 0.5) { Text("Time limit") }
                }
            }
            if flow.draft != nil {
                KitSection("What it costs") {
                    HStack(spacing: Space.xs) {
                        Text("Sandbox").foregroundStyle(Ink.textMuted)
                        Text(Money.exact(flow.estimatedSandboxUSD)).kitType(.mono).foregroundStyle(Ink.text)
                        Text("for " + String(format: "%g", flow.timeLimitHours) + " h, inside your " + Money.short(flow.budgetUSD) + " budget.")
                            .foregroundStyle(Ink.textMuted)
                    }
                    .kitType(.body)
                }
            }
        }
    }

    private func limit<Control: View>(
        title: String, value: String, unit: String? = nil, note: String, @ViewBuilder control: () -> Control
    ) -> some View {
        KitSection(title) {
            VStack(alignment: .leading, spacing: Space.md) {
                HStack(alignment: .firstTextBaseline, spacing: Space.md) {
                    HStack(alignment: .firstTextBaseline, spacing: Space.xs) {
                        Text(value).kitType(.hero).tracking(-1).foregroundStyle(Ink.text)
                        if let unit { Text(unit).kitType(.heroUnit).foregroundStyle(Ink.textMuted) }
                    }
                    control().labelsHidden()
                }
                Text(note).kitType(.meta).foregroundStyle(Ink.textFaint)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
