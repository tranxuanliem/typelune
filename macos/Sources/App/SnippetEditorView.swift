import SwiftUI

struct SnippetEditorView: View {
    @Environment(AppModel.self) private var model
    let snippetID: String

    // Seeded once per snippet: the parent gives this view `.id(snippetID)`.
    @State private var tagsText = ""
    @State private var bodySelection: TextSelection?
    @State private var preview: RenderResult?

    var body: some View {
        if let snippet = model.snippet(snippetID) {
            Form {
                expansionSection(snippet)
                detailsSection(snippet)
                bodySection(snippet)
                PreviewSection(preview: preview)
            }
            .formStyle(.grouped)
            .navigationTitle(snippet.displayName)
            .onAppear { tagsText = snippet.tags.joined(separator: ", ") }
            .task(id: snippet.body) {
                // Rendered by the Rust template engine; re-rendered when the body changes.
                preview = model.render(snippet.body)
            }
        }
    }

    @ViewBuilder
    private func expansionSection(_ snippet: Snippet) -> some View {
        Section {
            TextField("Abbreviation", text: model.binding(snippetID, \.abbreviation), prompt: Text("e.g. sig"))
                .font(.body.monospaced())
                .onChange(of: snippet.abbreviation) { _, new in
                    if new.count > 48 { model.binding(snippetID, \.abbreviation).wrappedValue = String(new.prefix(48)) }
                }
            Picker("Expands", selection: model.binding(snippetID, \.trigger, debounce: false)) {
                ForEach(SnippetTrigger.allCases) { trigger in
                    Text(trigger.title).tag(trigger)
                }
            }
            Toggle("Case-sensitive", isOn: model.binding(snippetID, \.caseSensitive, debounce: false))
            Toggle("Enabled", isOn: model.binding(snippetID, \.enabled, debounce: false))
            ForEach(Array(model.lints(for: snippetID).enumerated()), id: \.offset) { _, message in
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .symbolRenderingMode(.multicolor)
                    .foregroundStyle(.secondary)
            }
            if !model.isGroupEnabled(containing: snippetID) {
                Label("This snippet's group is disabled, so it won't expand.", systemImage: "folder.badge.minus")
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("Expansion")
        } footer: {
            Text(
                "Effective trigger: \(snippet.trigger.resolved(default: model.library.settings.defaultTrigger).title)."
            )
            .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func detailsSection(_ snippet: Snippet) -> some View {
        Section("Details") {
            TextField("Title", text: model.binding(snippetID, \.title), prompt: Text("Optional"))
            TextField("Tags", text: $tagsText, prompt: Text("Comma-separated"))
                .onSubmit { model.setTags(snippetID, from: tagsText) }
                .onChange(of: tagsText) { _, new in model.setTags(snippetID, from: new) }
            Picker(
                "Group",
                selection: Binding(
                    get: { model.groupID(containing: snippetID) ?? "" },
                    set: { model.move(snippetID, toGroup: $0) })
            ) {
                ForEach(model.groups) { group in
                    Text(group.name).tag(group.id)
                }
            }
        }
    }

    @ViewBuilder
    private func bodySection(_ snippet: Snippet) -> some View {
        Section {
            TextEditor(text: model.binding(snippetID, \.body), selection: $bodySelection)
                .font(.body.monospaced())
                .frame(minHeight: 140)
                .accessibilityLabel("Snippet text")
        } header: {
            HStack {
                Text("Text")
                Spacer()
                Menu("Insert", systemImage: "plus.circle") {
                    Button("Date") { insert("{{date}}") }
                    Button("Time") { insert("{{time}}") }
                    Button("Date and Time") { insert("{{datetime}}") }
                    Button("Custom Date Format") { insert("{{date:%d %b %Y}}") }
                    Divider()
                    Button("Clipboard") { insert("{{clipboard}}") }
                    Button("Cursor Position") { insert("{{cursor}}") }
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Insert a placeholder at the cursor")
            }
        } footer: {
            Text("Plain text. Placeholders are listed in Settings ▸ Placeholders.")
                .foregroundStyle(.secondary)
        }
    }

    /// Insert a placeholder at the editor's caret (or replace the selection).
    private func insert(_ token: String) {
        let text = model.binding(snippetID, \.body)
        var body = text.wrappedValue
        var offset = body.count
        if let selection = bodySelection, case .selection(let range) = selection.indices,
            range.lowerBound >= body.startIndex, range.upperBound <= body.endIndex
        {
            offset = body.distance(from: body.startIndex, to: range.lowerBound)
            body.replaceSubrange(range, with: token)
        } else {
            body += token
        }
        text.wrappedValue = body
        let caret = body.index(body.startIndex, offsetBy: offset + token.count)
        bodySelection = TextSelection(insertionPoint: caret)
    }
}

private struct PreviewSection: View {
    let preview: RenderResult?

    var body: some View {
        Section {
            if let preview {
                Group {
                    if preview.text.isEmpty && !preview.hasCursor {
                        Text("Empty").foregroundStyle(.secondary)
                    } else if preview.hasCursor {
                        Text("\(preview.beforeCursor)\(Text("▏").foregroundStyle(.tint).bold())\(preview.afterCursor)")
                    } else {
                        Text(preview.text)
                    }
                }
                .font(.body.monospaced())
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                ForEach(Array(preview.warnings.enumerated()), id: \.offset) { _, warning in
                    Label(warning, systemImage: "exclamationmark.triangle.fill")
                        .symbolRenderingMode(.multicolor)
                        .foregroundStyle(.secondary)
                }
            } else {
                Text("Preview unavailable").foregroundStyle(.secondary)
            }
        } header: {
            Text("Preview")
        } footer: {
            if preview?.hasCursor == true {
                Text("▏ marks where the caret lands after expansion.").foregroundStyle(.secondary)
            }
        }
    }
}
