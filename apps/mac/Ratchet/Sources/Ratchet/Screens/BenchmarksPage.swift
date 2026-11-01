import RatchetKit
import SwiftUI

extension View {
    /// A hairline card: 16 pt padding, 12 pt corners.
    func kitCard() -> some View {
        padding(Space.lg)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).fill(Ink.raised.opacity(0.4)))
            .overlay(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous)
                .strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
    }
}

/// Benchmark history, then the gallery of templates and demos.
struct BenchmarksPage: View {
    @Environment(AppStore.self) private var store
    @Environment(AppRouter.self) private var router

    private let columns = [GridItem(.adaptive(minimum: 280), spacing: Space.md, alignment: .top)]

    var body: some View {
        KitPage {
            PageHeader("Benchmarks", subtitle: "What each optimization was measured on, and templates to start from.")
                .gridProbe("header", area: "benchmarks")
            KitSection("History") { history }
                .gridProbe("history", area: "benchmarks")
            BenchmarkRunSection()
                .gridProbe("run", area: "benchmarks")
            KitSection("Templates") { gallery(.template) }
                .gridProbe("templates", area: "benchmarks")
            KitSection("Try a demo") { gallery(.demo) }
                .gridProbe("demos", area: "benchmarks")
        }
        .gridArea("benchmarks")
    }

    // MARK: History

    private var history: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Space.lg) {
                column("Benchmark").frame(maxWidth: .infinity, alignment: .leading)
                column("Result").frame(width: TableColumns.result, alignment: .leading)
                column("Noise").frame(width: 72, alignment: .trailing)
                column("Evals").frame(width: 64, alignment: .trailing)
                column("Started").frame(width: 80, alignment: .trailing)
            }
            .padding(.bottom, Space.sm + 2)
            .overlay(alignment: .bottom) { Hairline() }
            ForEach(store.benchmarks) { run in
                HStack(spacing: Space.lg) {
                    TitleStack(title: run.title, subtitle: run.optimizationTitle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    result(run).frame(width: TableColumns.result, alignment: .leading)
                    Text(Percent.noise(run.noise)).kitType(.mono).foregroundStyle(Ink.textMuted)
                        .frame(width: 72, alignment: .trailing)
                    Text("\(run.evals)").kitType(.mono).foregroundStyle(Ink.text)
                        .frame(width: 64, alignment: .trailing)
                    Text(Elapsed.shortDate(run.date)).kitType(.meta).foregroundStyle(Ink.textFaint)
                        .frame(width: 80, alignment: .trailing)
                }
                .padding(.vertical, Dim.rowPadding)
                .overlay(alignment: .bottom) { Hairline() }
            }
        }
    }

    private func column(_ title: String) -> some View {
        Text(title).kitType(.caption).foregroundStyle(Ink.textFaint)
    }

    @ViewBuilder
    private func result(_ run: BenchmarkRun) -> some View {
        if let change = run.change, let best = run.best {
            HStack(spacing: Space.sm) {
                Text(Percent.signed(change)).kitType(.mono).foregroundStyle(run.isImprovement ? Ink.ok : Ink.bad)
                Text(run.metric.pair(from: run.baseline, to: best)).kitType(.meta).foregroundStyle(Ink.textFaint).lineLimit(1)
            }
        } else {
            Text("baseline " + run.metric.text(run.baseline)).kitType(.meta).foregroundStyle(Ink.textFaint)
        }
    }

    // MARK: Gallery

    private func gallery(_ kind: TemplateKind) -> some View {
        LazyVGrid(columns: columns, alignment: .leading, spacing: Space.md) {
            ForEach(store.templates.filter { $0.kind == kind }) { template in
                card(template)
            }
        }
    }

    private func card(_ template: Template) -> some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            HStack {
                Text(template.title).kitType(.bodyStrong).foregroundStyle(Ink.text)
                Spacer(minLength: Space.sm)
                Text(template.category).kitType(.caption).foregroundStyle(Ink.textFaint)
            }
            Text(template.summary).kitType(.meta).foregroundStyle(Ink.textMuted)
                .fixedSize(horizontal: false, vertical: true)
            Text("Measures " + template.measures + " · " + template.machine)
                .kitType(.caption).foregroundStyle(Ink.textFaint)
            Spacer(minLength: Space.xs)
            Button(template.kind == .demo ? "Try a demo" : "Use template") {
                if template.kind == .demo {
                    router.startWizard(client: store.client, templateID: template.id)
                } else {
                    router.templateProposal = TemplateProposalFlow(template: template, client: store.client)
                }
            }
            .buttonStyle(.kit(.quiet, small: true))
        }
        .kitCard()
    }
}
