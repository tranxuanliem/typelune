import SwiftUI

/// The snippet editor: name, abbreviation and enabled state on top, then a large plain-text
/// writing surface. Occasional settings (group, tags, trigger, case sensitivity) live in the
/// Options popover and the rendered preview is a disclosure, so the text is always in reach.
struct SnippetEditorView: View {
    @Environment(AppModel.self) private var model
    let snippetID: String

    // Seeded once per snippet: the parent gives this view `.id(snippetID)`.
    @State private var tagsText = ""
    @State private var bodySelection: TextSelection?
    @State private var preview: RenderResult?
    @State private var showsOptions = false
    @AppStorage("editor.showsPreview") private var showsPreview = false

    var body: some View {
        if let snippet = model.snippet(snippetID) {
            // The notices and preview get explicit, bounded heights taken from the editor's own
            // container (not the screen, see `EditorHeights`) and scroll when they need more, so
            // the text editor keeps at least its floor and the footer stays reachable. The
            // editor is never rebuilt on resize.
            GeometryReader { geometry in
                VStack(spacing: 0) {
                    header(snippet)
                    Divider()
                    bodyEditor
                    notices(maxHeight: EditorHeights.noticeCap(containerHeight: geometry.size.height))
                    if showsPreview {
                        Divider()
                        PreviewPane(
                            preview: preview,
                            maxHeight: EditorHeights.previewCap(containerHeight: geometry.size.height))
                    }
                    Divider()
                    footer(snippet)
                }
                .frame(width: geometry.size.width, height: geometry.size.height)
            }
            .navigationTitle(snippet.displayName)
            .onAppear { tagsText = snippet.tags.joined(separator: ", ") }
            .task(id: snippet.body) {
                // Rendered by the Rust template engine; re-rendered when the body changes.
                preview = model.render(snippet.body)
            }
        }
    }

    // MARK: Header

    private func header(_ snippet: Snippet) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField("Title", text: model.binding(snippetID, \.title), prompt: Text("Untitled"))
                .textFieldStyle(.plain)
                .font(.title2.weight(.semibold))
                .accessibilityLabel("Title")
            HStack(spacing: 12) {
                HStack(spacing: 8) {
                    Text("Type")
                        .foregroundStyle(.secondary)
                    TextField("Abbreviation", text: model.binding(snippetID, \.abbreviation), prompt: Text("e.g. sig"))
                        .textFieldStyle(.roundedBorder)
                        .font(.body.monospaced())
                        .frame(width: 150)
                        .accessibilityLabel("Abbreviation")
                        .onChange(of: snippet.abbreviation) { _, new in
                            if new.count > 48 {
                                model.binding(snippetID, \.abbreviation).wrappedValue = String(new.prefix(48))
                            }
                        }
                }
                Spacer(minLength: 12)
                Toggle("Enabled", isOn: model.binding(snippetID, \.enabled, debounce: false))
                    .toggleStyle(.switch)
                    .controlSize(.small)
            }
        }
        .padding(.horizontal, 24)
        .padding(.top, 20)
        .padding(.bottom, 16)
    }

    // MARK: Body

    private var bodyEditor: some View {
        TextEditor(text: model.binding(snippetID, \.body), selection: $bodySelection)
            .font(.body)
            .scrollContentBackground(.hidden)
            .padding(.horizontal, 19)  // aligns the text with the header (TextEditor adds ~5pt)
            .padding(.vertical, 8)
            .frame(minHeight: EditorHeights.bodyFloor, maxHeight: .infinity)
            .accessibilityLabel("Snippet text")
    }

    /// Distinct warning messages in first-seen order (snippet lints, then template warnings).
    private var warningMessages: [String] {
        var seen = Set<String>()
        return (model.lints(for: snippetID) + (preview?.warnings ?? [])).filter { seen.insert($0).inserted }
    }

    /// Warnings that must not be missed: shown whether or not the preview is open. Taller
    /// lists scroll inside `maxHeight`, under a one-line summary.
    @ViewBuilder
    private func notices(maxHeight: CGFloat) -> some View {
        let messages = warningMessages
        let groupDisabled = !model.isGroupEnabled(containing: snippetID)
        if !messages.isEmpty || groupDisabled {
            VStack(alignment: .leading, spacing: 4) {
                if messages.count > 1 {
                    Text("\(messages.count) warnings")
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
                BoundedScroll(maxHeight: maxHeight) {
                    noticeList(messages, groupDisabled: groupDisabled)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 24)
            .padding(.vertical, EditorHeights.noticePadding / 2)
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Warnings")
        }
    }

    private func noticeList(_ messages: [String], groupDisabled: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(messages, id: \.self) { message in
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .symbolRenderingMode(.multicolor)
            }
            if groupDisabled {
                Label("This snippet's group is disabled, so it won't expand.", systemImage: "folder.badge.minus")
            }
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: Footer

    private func footer(_ snippet: Snippet) -> some View {
        HStack(spacing: 12) {
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
            .help("Insert a placeholder at the cursor. Placeholders are listed in Settings ▸ Placeholders.")
            Spacer()
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { showsPreview.toggle() }
            } label: {
                Label("Preview", systemImage: showsPreview ? "chevron.down" : "chevron.right")
                    .labelStyle(.titleAndIcon)
            }
            .buttonStyle(.borderless)
            .accessibilityValue(showsPreview ? "Shown" : "Hidden")
            .help(showsPreview ? "Hide the rendered preview" : "Show the text as it will be typed")
            Button {
                showsOptions.toggle()
            } label: {
                Label("Options", systemImage: "slider.horizontal.3")
                    .labelStyle(.titleAndIcon)
            }
            .buttonStyle(.borderless)
            .help("Group, tags, trigger and case sensitivity")
            .popover(isPresented: $showsOptions, arrowEdge: .top) { optionsPopover(snippet) }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 8)
    }

    private func optionsPopover(_ snippet: Snippet) -> some View {
        Form {
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
            TextField("Tags", text: $tagsText, prompt: Text("Comma-separated"))
                .onSubmit { model.setTags(snippetID, from: tagsText) }
                .onChange(of: tagsText) { _, new in model.setTags(snippetID, from: new) }
            Picker("Expands", selection: model.binding(snippetID, \.trigger, debounce: false)) {
                ForEach(SnippetTrigger.allCases) { trigger in
                    Text(trigger.title).tag(trigger)
                }
            }
            Toggle("Case-sensitive", isOn: model.binding(snippetID, \.caseSensitive, debounce: false))
            Text(
                "Effective trigger: \(snippet.trigger.resolved(default: model.library.settings.defaultTrigger).title)."
            )
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .formStyle(.columns)
        .padding(16)
        .frame(width: 320)
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

/// The text as it will be typed (date, time and cursor rendered by the Rust engine).
private struct PreviewPane: View {
    let preview: RenderResult?
    let maxHeight: CGFloat

    var body: some View {
        BoundedScroll(maxHeight: maxHeight) { content }
            .background(.quaternary.opacity(0.4))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Preview")
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let preview {
                if preview.text.isEmpty && !preview.hasCursor {
                    Text("Empty").foregroundStyle(.secondary)
                } else if preview.hasCursor {
                    Text("\(preview.beforeCursor)\(Text("▏").foregroundStyle(.tint).bold())\(preview.afterCursor)")
                        .textSelection(.enabled)
                    Text("▏ marks where the caret lands after expansion.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else {
                    Text(preview.text).textSelection(.enabled)
                }
            } else {
                Text("Preview unavailable").foregroundStyle(.secondary)
            }
        }
        .font(.body)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 24)
        .padding(.vertical, 12)
    }
}

/// A vertical scroll view that is as tall as its content up to `maxHeight`, then scrolls.
/// The content's height is measured, and the frame is explicit: unlike a flexible scroll
/// view in a stack, it can neither collapse to nothing nor take all the space.
private struct BoundedScroll<Content: View>: View {
    let maxHeight: CGFloat
    @ViewBuilder let content: Content
    @State private var natural: CGFloat?

    var body: some View {
        ScrollView {
            content
                .onGeometryChange(for: CGFloat.self) {
                    $0.size.height
                } action: {
                    natural = $0
                }
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(height: EditorHeights.boundedHeight(natural: natural, cap: maxHeight))
    }
}

/// Sizes of the editor's secondary areas. They get explicit, bounded heights taken from the
/// editor's own container, so they can neither collapse nor starve the text editor.
private enum EditorHeights {
    static let bodyFloor: CGFloat = 96
    static let noticePadding: CGFloat = 16

    static func noticeCap(containerHeight: CGFloat) -> CGFloat {
        max(0, min(72, containerHeight * 0.13))
    }

    static func previewCap(containerHeight: CGFloat) -> CGFloat {
        max(0, min(140, containerHeight * 0.2))
    }

    /// The content's height when it fits, otherwise the cap (and the area scrolls). Before the
    /// content is measured, the cap is used so the area is never blank.
    static func boundedHeight(natural: CGFloat?, cap: CGFloat) -> CGFloat {
        min(max(natural ?? cap, 0), cap)
    }
}
