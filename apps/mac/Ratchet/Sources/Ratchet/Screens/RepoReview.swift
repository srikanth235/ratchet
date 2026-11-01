import RatchetKit
import SwiftUI

// What step 1 learned about the chosen repo: the review of ratchet.yaml, why it cannot be used, the templates that
// add one through a pull request. And "What's missing" on the Connections step.

struct RepoReview: View {
    @Bindable var flow: NewOptimizationFlow

    var body: some View {
        if flow.isInspecting {
            HStack(spacing: Space.sm) {
                ProgressView().controlSize(.small)
                Text("Reading ratchet.yaml…").kitType(.body).foregroundStyle(Ink.textMuted)
            }
        } else if let failure = flow.inspectionError {
            NoteLine(tone: .bad, text: failure)
        } else if let inspection = flow.inspection {
            VStack(alignment: .leading, spacing: Space.lg) {
                switch inspection.outcome {
                case .unusable(let why): NoteLine(tone: .bad, text: why)
                case .ready: ready(inspection)
                case .invalid: invalid(inspection)
                case .missing: missing(inspection)
                }
                templateLink(inspection)
            }
        } else {
            Text("Choose a repo to look for a benchmark.").kitType(.body).foregroundStyle(Ink.textFaint)
        }
    }

    // MARK: ratchet.yaml found

    private func ready(_ inspection: RepoInspection) -> some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            NoteLine(tone: .ok, text: foundLine(inspection))
            if let config = inspection.config {
                HairlineList {
                    fact("Measured", config.metricName + " · " + config.direction)
                    fact("Evaluator", evaluator(config))
                    fact("Toolchain", config.image ?? "the bench machine itself (no container)")
                    fact("Machine", config.machine + " · " + (config.region ?? "default region"))
                    fact("Calibration", config.repeats.map { "\($0) runs per candidate, set by the repo" } ?? "measured from the noise during the baseline")
                }
                if !config.protected.isEmpty { protected(config) }
            }
            if let yaml = inspection.configYAML {
                DisclosureGroup(isExpanded: $flow.showsYAML) {
                    VStack(alignment: .leading, spacing: Space.sm) {
                        Text(yaml)
                            .kitType(.mono)
                            .foregroundStyle(Ink.textMuted)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .setupPanel()
                            .accessibilityLabel("ratchet.yaml, read-only")
                        Text("Read-only: the repo owns this file. Change it through a pull request.")
                            .kitType(.caption).foregroundStyle(Ink.textFaint)
                    }
                    .padding(.top, Space.sm)
                } label: {
                    Text("View ratchet.yaml").kitType(.meta).foregroundStyle(Ink.textMuted)
                }
            }
        }
    }

    private func foundLine(_ inspection: RepoInspection) -> String {
        let name = inspection.config?.metricName ?? ""
        let direction = inspection.config?.direction ?? ""
        return "Found `ratchet.yaml` on \(inspection.baseRef) of \(inspection.repo ?? ""): \(name) (\(direction))."
    }

    private func evaluator(_ config: RepoConfig) -> String {
        var line = config.evaluatorIsTemplate ? "template " + config.evaluator : config.evaluator
        if let holdout = config.holdout { line += " · " + holdout }
        return line
    }

    private func fact(_ label: String, _ value: String) -> some View {
        HairlineRow(verticalPadding: Dim.rowPaddingTight) {
            Text(label).kitType(.meta).foregroundStyle(Ink.textFaint).frame(width: 120, alignment: .leading)
            Text(value).kitType(.body).foregroundStyle(Ink.text).frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func protected(_ config: RepoConfig) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
            Text("Protected from agents").kitType(.meta).foregroundStyle(Ink.textFaint)
            ForEach(config.protected) { path in
                Text(path.glob)
                    .kitType(.mono)
                    .foregroundStyle(Ink.textMuted)
                    .help(path.reason)
            }
        }
    }

    // MARK: ratchet.yaml does not parse

    private func invalid(_ inspection: RepoInspection) -> some View {
        VStack(alignment: .leading, spacing: Space.md) {
            NoteLine(tone: .bad, text: "`ratchet.yaml` in \(inspection.repo ?? "this repo") does not parse. Fix it through a pull request:")
            VStack(alignment: .leading, spacing: Space.sm) {
                ForEach(inspection.problems) { problem in
                    HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
                        if !problem.path.isEmpty {
                            Text(problem.path).kitType(.mono).foregroundStyle(Ink.text)
                        }
                        Text(problem.message).kitType(.meta).foregroundStyle(Ink.textMuted)
                    }
                }
            }
            .padding(.leading, Space.xxl)
        }
    }

    // MARK: No ratchet.yaml: the templates

    private func missing(_ inspection: RepoInspection) -> some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            NoteLine(
                tone: .wait,
                text: "\(inspection.repo ?? "This repo") has no `ratchet.yaml` on \(inspection.baseRef). Pick a template and Ratchet opens a pull request that adds it. The repo owns the config; changes go through a pull request.")
            if let failure = flow.proposalError {
                Text(failure).kitType(.meta).foregroundStyle(Ink.bad)
            }
            if let proposal = flow.proposal {
                PullRequestResult(proposal: proposal)
                Button("I merged it: check again") { Task { await flow.recheck() } }
                    .buttonStyle(.kit(.quiet))
            }
            HairlineList {
                ForEach(inspection.templates) { template in
                    HairlineRow(verticalPadding: Dim.rowPaddingTight) {
                        TitleStack(title: template.title, subtitle: template.summary + " · " + template.measures, subtitleLines: 2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button(flow.proposingTemplateID == template.id ? "Opening…" : "Add via pull request") {
                            Task { await flow.proposeTemplate(template.id) }
                        }
                        .buttonStyle(.kit(.quiet, small: true))
                        .disabled(flow.proposingTemplateID != nil)
                        .accessibilityLabel("Add \(template.title) via pull request")
                    }
                }
            }
        }
    }

    // MARK: Start from a template instead

    @ViewBuilder
    private func templateLink(_ inspection: RepoInspection) -> some View {
        let detection = inspection.harness
        if let id = detection.suggestedTemplateID, let template = flow.templates.first(where: { $0.id == id }) {
            if flow.templateID == id {
                Text("Starting from the " + template.title + " template.").kitType(.meta).foregroundStyle(Ink.textFaint)
            } else {
                let verb = detection.found ? "Use the " : "Start from the "
                let suffix = detection.found ? " template instead" : " template"
                Button(verb + template.title + suffix) { flow.useTemplate(id) }
                    .buttonStyle(.kit(.link))
            }
        }
    }
}

/// "What's missing" on the Connections step: what the daemon says would stop this run, or that nothing does.
struct WhatsMissing: View {
    @Bindable var flow: NewOptimizationFlow
    @Environment(AppRouter.self) private var router
    @Environment(\.openSettings) private var openSettings

    private var shown: [PreflightItem] { flow.preflight.filter { $0.state != .ok } }

    var body: some View {
        KitSection("What's missing") {
            if shown.isEmpty {
                HStack(spacing: Space.sm) {
                    KitIconView(icon: .check, size: Dim.icon, weight: 1.6).foregroundStyle(Ink.ok)
                    Text("Everything this run needs is in place.").kitType(.body).foregroundStyle(Ink.textMuted)
                }
            } else {
                VStack(alignment: .leading, spacing: Space.md) {
                    ForEach(shown) { item in
                        HStack(alignment: .top, spacing: Space.sm) {
                            StatusDot(tone: item.state == .fail ? .bad : .wait).frame(height: Dim.textLine)
                            VStack(alignment: .leading, spacing: 0) {
                                Text(item.label).kitType(.bodyStrong).foregroundStyle(Ink.text)
                                CommandText(text: item.detail)
                            }
                        }
                        .accessibilityElement(children: .combine)
                    }
                    if !flow.blockers.isEmpty {
                        Button("Open Connectors") {
                            router.settingsTab = .connectors
                            openSettings()
                        }
                        .buttonStyle(.kit(.quiet, small: true))
                    }
                }
            }
        }
    }
}
