import SwiftUI

/// Preview of a CSV import. Nothing changes until Merge or Replace is chosen.
struct CsvImportView: View {
    @Environment(AppModel.self) private var model
    let pending: PendingImport

    private var report: CsvReport? { pending.report }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Import “\(pending.fileName)”").font(.headline)
            if let r = report {
                Text(
                    "\(r.imported) of \(r.rows) rows can be imported into \(pending.summary.groups) groups. \(r.skippedCount) skipped, \(r.warningCount) warnings."
                )
                if !r.ignoredColumns.isEmpty {
                    Text("Ignored columns: \(r.ignoredColumns.joined(separator: ", "))")
                        .foregroundStyle(.secondary)
                }
                List {
                    if !r.skipped.isEmpty {
                        Section("Skipped rows") {
                            ForEach(r.skipped, id: \.self) { issue in issueRow(issue, symbol: "xmark.circle") }
                            if r.skippedCount > r.skipped.count {
                                Text("… and \(r.skippedCount - r.skipped.count) more").foregroundStyle(.secondary)
                            }
                        }
                    }
                    if !r.warnings.isEmpty {
                        Section("Warnings") {
                            ForEach(r.warnings, id: \.self) { issue in
                                issueRow(issue, symbol: "exclamationmark.triangle")
                            }
                            if r.warningCount > r.warnings.count {
                                Text("… and \(r.warningCount - r.warnings.count) more").foregroundStyle(.secondary)
                            }
                        }
                    }
                    Section("Snippets") {
                        ForEach(pending.library.groups) { g in
                            ForEach(g.snippets.prefix(100)) { s in
                                HStack {
                                    Text(s.abbreviation).font(.body.monospaced())
                                    Text(s.displayName).foregroundStyle(.secondary).lineLimit(1)
                                    Spacer()
                                    Text(g.name).font(.caption).foregroundStyle(.secondary)
                                }
                            }
                        }
                    }
                }
                .frame(minHeight: 240)
            }
            Text(
                "Text is imported as typed; placeholders Typelune does not support stay as literal text. Merging adds new groups; replacing swaps the whole library (your settings are kept). Both can be undone."
            )
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { model.pendingImport = nil }
                    .keyboardShortcut(.cancelAction)
                // This preview is the confirmation: the button acts immediately (Undo works).
                Button("Replace Library", role: .destructive) { model.confirmImport(pending, replace: true) }
                    .disabled(!hasRows)
                    .help(hasRows ? "Replace the whole library with these snippets" : "Nothing to import")
                Button("Merge into Library") { model.confirmImport(pending, replace: false) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!hasRows)
            }
        }
        .padding()
        .frame(width: 620, height: 520)
    }

    /// A CSV with no importable rows never offers Replace (or an empty Merge).
    private var hasRows: Bool { (report?.imported ?? 0) > 0 }

    private func issueRow(_ issue: CsvIssue, symbol: String) -> some View {
        Label(issue.line > 0 ? "Line \(issue.line): \(issue.message)" : issue.message, systemImage: symbol)
            .font(.callout)
    }
}
