import RatchetKit
import SwiftUI

// Small parts of the setup screens: command text, form blocks, the status line, the pull request result.

/// "run `codex login`": backticks mark a command, shown in monospace.
struct CommandText: View {
    let text: String
    var type: KitType = .meta
    var color: Color = Ink.textFaint

    var body: some View {
        Text(attributed)
            .kitType(type)
            .foregroundStyle(color)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var attributed: AttributedString {
        var result = AttributedString()
        for (index, part) in text.split(separator: "`", omittingEmptySubsequences: false).enumerated() {
            var piece = AttributedString(String(part))
            if index % 2 == 1 {
                piece.font = KitType.mono.font
                piece.foregroundColor = Ink.textMuted
            }
            result += piece
        }
        return result
    }
}

/// A dot and one line: the result of a check, or a note about a state.
struct NoteLine: View {
    let tone: StatusTone
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.sm) {
            StatusDot(tone: tone)
            CommandText(text: text, type: .body, color: Ink.textMuted)
        }
        .accessibilityElement(children: .combine)
    }
}

/// A label over a field, with a hint under it that turns into the problem when there is one.
struct FieldBlock<Content: View>: View {
    let label: String
    var hint: String?
    var problem: String?
    let content: Content

    init(_ label: String, hint: String? = nil, problem: String? = nil, @ViewBuilder content: () -> Content) {
        self.label = label
        self.hint = hint
        self.problem = problem
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            Text(label).kitType(.captionStrong).foregroundStyle(Ink.textMuted)
            content
            if let problem {
                Text(problem).kitType(.caption).foregroundStyle(Ink.bad).fixedSize(horizontal: false, vertical: true)
            } else if let hint {
                Text(hint).kitType(.caption).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// The pull request a template added: where it is and what it changed.
struct PullRequestResult: View {
    let proposal: TemplateProposal

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            HStack(spacing: Space.sm) {
                KitIconView(icon: .check, size: Dim.icon, weight: 1.6).foregroundStyle(Ink.ok)
                Text("Pull request opened").kitType(.bodyStrong).foregroundStyle(Ink.text)
            }
            if let url = proposal.url {
                Link(proposal.prURL, destination: url)
                    .kitType(.mono)
                    .foregroundStyle(Ink.brassText)
                    .lineLimit(1)
            } else {
                Text(proposal.prURL).kitType(.mono).foregroundStyle(Ink.textMuted)
            }
            Text(proposal.summary).kitType(.meta).foregroundStyle(Ink.textFaint).fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Pull request opened. " + proposal.prURL)
    }
}

/// A hairline card around a block of settings.
extension View {
    func setupPanel() -> some View {
        padding(Space.lg)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).fill(Ink.raised.opacity(0.4)))
            .overlay(RoundedRectangle(cornerRadius: Dim.radiusCard, style: .continuous).strokeBorder(Ink.hairline, lineWidth: Dim.hairline))
    }
}
