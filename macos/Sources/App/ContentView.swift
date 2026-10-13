import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.undoManager) private var undoManager
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 180, ideal: 220, max: 320)
        } content: {
            SnippetListView()
                .navigationSplitViewColumnWidth(min: 240, ideal: 300, max: 440)
        } detail: {
            if let id = model.snippetSelection, model.snippet(id) != nil {
                SnippetEditorView(snippetID: id)
                    .id(id)  // fresh editor state (tags text, selection) per snippet
            } else {
                ContentUnavailableView {
                    Label("No Snippet Selected", systemImage: "text.cursor")
                } description: {
                    Text("Select a snippet, or create one with ⌘N.")
                } actions: {
                    Button("New Snippet") { model.newSnippet() }
                }
            }
        }
        .toolbar { MainToolbar() }
        .onAppear { model.undoManager = undoManager }
        .onChange(of: undoManager) { _, newValue in model.undoManager = newValue }
        .fileImporter(isPresented: $model.showImporter, allowedContentTypes: [.json, .commaSeparatedText]) { result in
            switch result {
            case .success(let url): model.beginImport(url)
            case .failure(let error): model.errorMessage = error.localizedDescription
            }
        }
        .fileDialogMessage("Choose a Typelune library export (JSON) or a CSV file with a header row.")
        .confirmationDialog(
            "Import Library",
            isPresented: Binding(
                get: { model.pendingImport != nil && model.pendingImport?.report == nil },
                set: { if !$0 { model.pendingImport = nil } }),
            presenting: model.pendingImport
        ) { pending in
            Button("Merge into Library") { model.confirmImport(pending, replace: false) }
            Button("Replace Library", role: .destructive) { model.confirmImport(pending, replace: true) }
            Button("Cancel", role: .cancel) { model.pendingImport = nil }
        } message: { pending in
            Text(
                "“\(pending.fileName)” contains \(pending.summary.groups) groups and \(pending.summary.snippets) snippets. Merging adds them as new groups; replacing swaps the whole library. Both can be undone."
            )
        }
        .sheet(
            item: Binding(
                get: { model.pendingImport?.report != nil ? model.pendingImport : nil },
                set: { if $0 == nil { model.pendingImport = nil } })
        ) { pending in
            CsvImportView(pending: pending)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if let message = model.statusMessage {
                HStack {
                    Image(systemName: "info.circle").foregroundStyle(.secondary)
                    Text(message).lineLimit(2)
                    Spacer()
                    Button("Dismiss", systemImage: "xmark") { model.statusMessage = nil }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                }
                .font(.callout)
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(.bar)
            }
        }
        .alert(
            "Something Went Wrong",
            isPresented: Binding(get: { model.errorMessage != nil }, set: { if !$0 { model.errorMessage = nil } })
        ) {
            Button("OK", role: .cancel) { model.errorMessage = nil }
        } message: {
            Text(model.errorMessage ?? "")
        }
    }
}

private struct MainToolbar: ToolbarContent {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            Button("New Snippet", systemImage: "square.and.pencil") { model.newSnippet() }
                .help("New Snippet (⌘N)")
            Menu("Library", systemImage: "ellipsis.circle") {
                Button("New Group", systemImage: "folder.badge.plus") { model.newGroup() }
                Divider()
                Button("Import Library…", systemImage: "square.and.arrow.down") { model.showImporter = true }
                Button("Export Library…", systemImage: "square.and.arrow.up") { model.exportLibrary() }
                Divider()
                Button("Backups…", systemImage: "clock.arrow.circlepath") { openWindow(id: WindowID.backups) }
            }
            .help("Groups, import and export")
            Button("Playground", systemImage: "keyboard") { openWindow(id: WindowID.playground) }
                .help("Try expansions in a scratch area (⌥⌘P)")
            Button {
                openWindow(id: WindowID.systemWide)
            } label: {
                Label(nativeTitle, systemImage: nativeSymbol)
            }
            .help("System-wide expansion: \(nativeTitle)")
        }
    }

    private var nativeTitle: String {
        "System-wide: \(model.native?.state.title ?? "Off")"
    }

    private var nativeSymbol: String {
        switch model.native?.state {
        case .running: "bolt.fill"
        case .paused: "pause.circle"
        case .error, .disabled: "exclamationmark.triangle"
        default: "bolt.slash"
        }
    }
}
