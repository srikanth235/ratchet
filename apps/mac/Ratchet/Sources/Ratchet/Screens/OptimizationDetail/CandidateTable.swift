import AppKit
import RatchetKit
import SwiftUI

/// Every candidate of a running or finished optimization with its verdict and measured value. The first 200 rows are laid out;
/// the rest wait behind "Show all".
struct CandidateTable: View {
    let optimization: Optimization
    let candidates: [Candidate]
    @State private var cap = RowCap()

    var body: some View {
        LazyVStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Space.lg) {
                columnTitle("Commit").frame(width: 76, alignment: .leading)
                columnTitle("Candidate").frame(maxWidth: .infinity, alignment: .leading)
                columnTitle("Verdict").frame(width: TableColumns.status, alignment: .leading)
                columnTitle(optimization.metric.name).frame(width: TableColumns.spent + Space.xl, alignment: .trailing)
            }
            .padding(.bottom, Space.sm + 2)
            .overlay(alignment: .bottom) { Hairline() }
            ForEach(cap.shown(candidates)) { candidate in
                HStack(spacing: Space.lg) {
                    Text(candidate.id).kitType(.mono).foregroundStyle(Ink.textFaint)
                        .frame(width: 76, alignment: .leading)
                    TitleStack(title: candidate.title, subtitle: candidate.agent)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    StatusLabel(tone: candidate.verdict.tone, text: candidate.verdict.title)
                        .frame(width: TableColumns.status, alignment: .leading)
                    Text(candidate.value.map(optimization.metric.text) ?? "—")
                        .kitType(.mono).foregroundStyle(candidate.value == nil ? Ink.textFaint : Ink.text)
                        .frame(width: TableColumns.spent + Space.xl, alignment: .trailing)
                }
                .padding(.vertical, Dim.rowPadding - 2)
                .overlay(alignment: .bottom) { Hairline() }
            }
            if let title = cap.buttonTitle(count: candidates.count, noun: "candidates") {
                Button(title) { cap.showsAll = true }
                    .buttonStyle(.kit(.quiet, small: true))
                    .padding(.top, Space.md)
            }
        }
    }

    private func columnTitle(_ text: String) -> some View {
        Text(text).kitType(.caption).foregroundStyle(Ink.textFaint)
    }
}
