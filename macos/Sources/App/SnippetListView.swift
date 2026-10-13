import SwiftUI

struct SnippetListView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        let rows = model.visibleRows
        List(rows, selection: $model.snippetSelection) { row in
            SnippetRowView(row: row, showsGroup: model.sidebarSelection == .all)
                .contextMenu {
                    Button("Duplicate") { model.duplicate(row.id) }
                    Button(row.active ? "Disable" : "Enable") { model.toggleEnabled(row.id) }
                    Divider()
                    Button("Delete", role: .destructive) { model.delete(row.id) }
                }
        }
        .onDeleteCommand {
            if let id = model.snippetSelection { model.delete(id) }
        }
        .searchable(text: $model.searchText, prompt: "Title, abbreviation, text or tag")
        .navigationTitle(model.selectedGroup?.name ?? "All Snippets")
        .navigationSubtitle("\(rows.count) snippets")
        .overlay {
            if rows.isEmpty {
                if model.searchText.isEmpty {
                    ContentUnavailableView {
                        Label("No Snippets", systemImage: "text.badge.plus")
                    } description: {
                        Text("Snippets turn a short abbreviation into longer text.")
                    } actions: {
                        Button("New Snippet") { model.newSnippet() }
                    }
                } else {
                    ContentUnavailableView.search(text: model.searchText)
                }
            }
        }
    }
}

private struct SnippetRowView: View {
    let row: SnippetRow
    let showsGroup: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(row.abbreviation.isEmpty ? "—" : row.abbreviation)
                .font(.body.monospaced())
                // Hierarchical styles let List adapt colours on the selection highlight.
                .foregroundStyle(row.active ? .primary : .secondary)
                .frame(minWidth: 56, alignment: .leading)
                .lineLimit(1)
            VStack(alignment: .leading, spacing: 2) {
                Text(row.name)
                    .foregroundStyle(row.active ? .primary : .secondary)
                    .lineLimit(1)
                if showsGroup {
                    Text(row.groupName)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if !row.active {
                Image(systemName: "pause.circle")
                    .foregroundStyle(.secondary)
                    .help("Disabled")
                    .accessibilityLabel("Disabled")
            }
            if row.hasLint {
                Image(systemName: "exclamationmark.triangle.fill")
                    .symbolRenderingMode(.multicolor)
                    .help("Has a warning")
                    .accessibilityLabel("Warning")
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}
