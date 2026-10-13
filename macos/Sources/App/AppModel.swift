// Main-actor app state. The Rust core owns matching, rendering, persistence and native
// expansion; this model keeps a Swift copy of the library for the UI, sends every accepted
// change back to Rust (which validates, re-indexes and queues the background save), and
// registers Undo for structural edits.

import AppKit
import Observation
import SwiftUI
import UniformTypeIdentifiers

enum SidebarItem: Hashable {
    case all
    case group(String)
}

struct SnippetRow: Identifiable, Equatable {
    let id: String
    let abbreviation: String
    let name: String
    let active: Bool
    let hasLint: Bool
    let groupName: String
}

struct PendingImport: Identifiable {
    let id = UUID()
    let fileName: String
    let library: Library
    let summary: MergeSummary
}

struct PlaygroundLogEntry: Identifiable, Equatable {
    let id = UUID()
    let time: Date
    let title: String
    let warnings: [String]
}

@Observable
@MainActor
final class AppModel {
    static let live = AppModel()

    @ObservationIgnored private let core: TypeluneCore?
    @ObservationIgnored private var commitTask: Task<Void, Never>?
    @ObservationIgnored private var lastCommitted: Library?
    @ObservationIgnored weak var undoManager: UndoManager?
    @ObservationIgnored private var layoutRefreshedAt: Date = .distantPast

    private(set) var library: Library {
        didSet { rebuildIndex() }
    }
    private(set) var lintsBySnippet: [String: [String]] = [:]
    private(set) var activeRules = 0
    private(set) var saveStatus: SaveStatus?
    private(set) var libraryPath = ""
    private(set) var loadInfo: LoadInfo?
    private(set) var coreVersion = ""
    private(set) var native: NativeStatus?
    /// A native command is running off the main actor (Stop joins threads, bounded by 2 s).
    /// While set, status polling is skipped and other native commands are disabled.
    private(set) var nativeBusy = false
    private(set) var playgroundLog: [PlaygroundLogEntry] = []
    private(set) var playgroundBuffer = ""

    var sidebarSelection: SidebarItem? = .all {
        didSet { if oldValue != sidebarSelection { ensureSelectionVisible() } }
    }
    var snippetSelection: String?
    /// Changing the query reconciles the selection to a visible match (or none). Ordinary
    /// edits to a snippet never do, even if the edited snippet stops matching the query.
    var searchText = "" {
        didSet { if oldValue != searchText { ensureSelectionVisible() } }
    }
    var errorMessage: String?
    var statusMessage: String?
    var showImporter = false
    var pendingImport: PendingImport?

    @ObservationIgnored private var index: [String: (group: Int, snippet: Int)] = [:]

    private init() {
        do {
            let core = try TypeluneCore()
            let state = try core.state()
            self.core = core
            self.library = state.library
            self.lastCommitted = state.library
            self.libraryPath = state.libraryPath
            self.loadInfo = state.load
            self.coreVersion = state.coreVersion
            self.activeRules = state.activeRules
            self.saveStatus = state.save
            applyLints(state.lints)
            switch state.load.kind {
            case "recovered":
                statusMessage = "The previous library could not be read and was moved aside."
            case "created_demo":
                statusMessage = "Created a library with example snippets."
            case "memory_only", "migration_failed":
                errorMessage = [state.load.error, state.save.lastError].compactMap { $0 }.joined(separator: "\n\n")
                    .ifEmpty("The library could not be opened; saving is disabled.")
            default:
                break
            }
            if state.load.migration?.kind == "copied" {
                statusMessage =
                    "Your library from the previous version was copied here. The original was left unchanged."
            }
            if state.load.deprecatedEnv == true {
                statusMessage = "ATEXT_RS_DATA_DIR is deprecated; use TYPELUNE_DATA_DIR."
            }
        } catch {
            self.core = nil
            self.library = Library(
                format: "atext-rs.library", version: 1,
                settings: LibrarySettings(
                    defaultTrigger: .delimiter, delimiters: " ", keepDelimiter: true, requireWordBoundary: true),
                groups: [])
            self.errorMessage = "Typelune could not open its library engine: \(error.localizedDescription)"
        }
        rebuildIndex()
        snippetSelection = library.groups.first?.snippets.first?.id
    }

    // MARK: Derived data

    var groups: [SnippetGroup] { library.groups }

    func snippet(_ id: String) -> Snippet? {
        guard let loc = index[id] else { return nil }
        return library.groups[loc.group].snippets[loc.snippet]
    }

    func groupID(containing snippetID: String) -> String? {
        index[snippetID].map { library.groups[$0.group].id }
    }

    func isGroupEnabled(containing snippetID: String) -> Bool {
        index[snippetID].map { library.groups[$0.group].enabled } ?? true
    }

    var selectedGroup: SnippetGroup? {
        guard case .group(let id) = sidebarSelection else { return nil }
        return library.groups.first { $0.id == id }
    }

    var visibleRows: [SnippetRow] {
        var rows: [SnippetRow] = []
        for g in library.groups {
            if case .group(let id) = sidebarSelection, id != g.id { continue }
            for s in g.snippets where s.matches(searchText) {
                rows.append(
                    SnippetRow(
                        id: s.id, abbreviation: s.abbreviation, name: s.displayName,
                        active: s.enabled && g.enabled, hasLint: lintsBySnippet[s.id] != nil,
                        groupName: g.name))
            }
        }
        return rows
    }

    func lints(for snippetID: String) -> [String] { lintsBySnippet[snippetID] ?? [] }

    // MARK: Editing

    /// Bind a snippet field. Text edits apply locally at once and are committed to Rust
    /// after a short pause, so typing does not re-validate the whole library per keystroke.
    func binding<Value>(_ snippetID: String, _ keyPath: WritableKeyPath<Snippet, Value>, debounce: Bool = true)
        -> Binding<Value>
    {
        Binding(
            get: { [weak self] in
                guard let self, let s = self.snippet(snippetID) else { return Snippet.new()[keyPath: keyPath] }
                return s[keyPath: keyPath]
            },
            set: { [weak self] newValue in
                guard let self, let loc = self.index[snippetID] else { return }
                self.library.groups[loc.group].snippets[loc.snippet][keyPath: keyPath] = newValue
                if debounce { self.scheduleCommit() } else { self.commitNow() }
            })
    }

    func settingsBinding<Value>(_ keyPath: WritableKeyPath<LibrarySettings, Value>) -> Binding<Value> {
        Binding(
            get: { [unowned self] in library.settings[keyPath: keyPath] },
            set: { [unowned self] newValue in
                library.settings[keyPath: keyPath] = newValue
                commitNow()
            })
    }

    private func scheduleCommit() {
        commitTask?.cancel()
        commitTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            self?.commitNow()
        }
    }

    /// Send the current library to Rust. If Rust rejects it, the last accepted library is
    /// restored and the reason shown.
    @discardableResult
    func commitNow() -> Bool {
        commitTask?.cancel()
        commitTask = nil
        guard let core else { return false }
        if library == lastCommitted { return true }
        do {
            let result = try core.update(library)
            lastCommitted = library
            activeRules = result.activeRules
            saveStatus = result.save
            applyLints(result.lints)
            return true
        } catch {
            errorMessage = error.localizedDescription
            if let lastCommitted { library = lastCommitted }
            return false
        }
    }

    /// Structural change with Undo/Redo registration.
    private func perform(_ actionName: String, _ change: (inout Library) -> Void) {
        commitNow()
        let before = library
        var after = library
        change(&after)
        guard after != before else { return }
        library = after
        if commitNow() {
            registerUndo(actionName, restoring: before)
        }
    }

    private func registerUndo(_ actionName: String, restoring snapshot: Library) {
        guard let undoManager else { return }
        undoManager.registerUndo(withTarget: self) { model in
            MainActor.assumeIsolated {
                let current = model.library
                model.library = snapshot
                if model.commitNow() {
                    model.registerUndo(actionName, restoring: current)
                    model.ensureSelectionVisible()
                }
            }
        }
        undoManager.setActionName(actionName)
    }

    func newSnippet() {
        let snippet = Snippet.new()
        perform("New Snippet") { lib in
            if lib.groups.isEmpty { lib.groups.append(.new(name: "General")) }
            let gi = targetGroupIndex(in: lib)
            lib.groups[gi].snippets.append(snippet)
        }
        searchText = ""
        snippetSelection = snippet.id
    }

    private func targetGroupIndex(in lib: Library) -> Int {
        if case .group(let id) = sidebarSelection, let i = lib.groups.firstIndex(where: { $0.id == id }) { return i }
        if let sel = snippetSelection, let loc = lib.locate(sel) { return loc.group }
        return 0
    }

    func newGroup() {
        var name = "New Group"
        var n = 2
        while library.groups.contains(where: { $0.name == name }) {
            name = "New Group \(n)"
            n += 1
        }
        let group = SnippetGroup.new(name: name)
        perform("New Group") { $0.groups.append(group) }
        sidebarSelection = .group(group.id)
    }

    func duplicate(_ snippetID: String) {
        guard let loc = index[snippetID] else { return }
        var copy = library.groups[loc.group].snippets[loc.snippet]
        copy.id = UUID().uuidString.lowercased()
        copy.title = "\(copy.displayName) Copy"
        copy.enabled = false  // avoids an immediate duplicate-abbreviation conflict
        perform("Duplicate Snippet") { $0.groups[loc.group].snippets.insert(copy, at: loc.snippet + 1) }
        snippetSelection = copy.id
        statusMessage = "The copy starts disabled so it doesn't clash with the original."
    }

    func toggleEnabled(_ snippetID: String) {
        guard let loc = index[snippetID] else { return }
        let enabled = library.groups[loc.group].snippets[loc.snippet].enabled
        perform(enabled ? "Disable Snippet" : "Enable Snippet") {
            $0.groups[loc.group].snippets[loc.snippet].enabled.toggle()
        }
    }

    func delete(_ snippetID: String) {
        guard let loc = index[snippetID] else { return }
        let rows = visibleRows.map(\.id)
        let next = rows.firstIndex(of: snippetID).flatMap { i -> String? in
            if i + 1 < rows.count { return rows[i + 1] }
            return i > 0 ? rows[i - 1] : nil
        }
        perform("Delete Snippet") { $0.groups[loc.group].snippets.remove(at: loc.snippet) }
        snippetSelection = next
    }

    func move(_ snippetID: String, toGroup groupID: String) {
        guard let loc = index[snippetID],
            let target = library.groups.firstIndex(where: { $0.id == groupID }),
            target != loc.group
        else { return }
        perform("Move Snippet") { lib in
            let s = lib.groups[loc.group].snippets.remove(at: loc.snippet)
            lib.groups[target].snippets.append(s)
        }
    }

    func setGroupEnabled(_ groupID: String, _ enabled: Bool) {
        guard let gi = library.groups.firstIndex(where: { $0.id == groupID }) else { return }
        perform(enabled ? "Enable Group" : "Disable Group") { $0.groups[gi].enabled = enabled }
    }

    func renameGroup(_ groupID: String, to name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let gi = library.groups.firstIndex(where: { $0.id == groupID }) else { return }
        perform("Rename Group") { $0.groups[gi].name = trimmed }
    }

    func deleteGroup(_ groupID: String) {
        guard let gi = library.groups.firstIndex(where: { $0.id == groupID }) else { return }
        perform("Delete Group") { $0.groups.remove(at: gi) }
        if sidebarSelection == .group(groupID) { sidebarSelection = .all }
        ensureSelectionVisible()
    }

    func setTags(_ snippetID: String, from text: String) {
        let tags = text.split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
            .prefix(32)
            .map { String($0.prefix(64)) }
        binding(snippetID, \.tags).wrappedValue = Array(tags)
    }

    private func ensureSelectionVisible() {
        let reconciled = SelectionRules.reconcile(snippetSelection, visible: visibleRows.map(\.id))
        if reconciled != snippetSelection { snippetSelection = reconciled }
    }

    // MARK: Rendering & Playground

    func render(_ body: String) -> RenderResult? {
        try? core?.render(body)
    }

    func playgroundEdit(backspaces: Int, inserted: String) -> PlaygroundExpansion? {
        commitNow()  // the playground must see the latest edits
        guard let result = try? core?.playgroundEdit(backspaces: backspaces, inserted: inserted) else { return nil }
        playgroundBuffer = result.buffer
        if let exp = result.expansion {
            playgroundLog.insert(PlaygroundLogEntry(time: .now, title: exp.title, warnings: exp.warnings), at: 0)
            if playgroundLog.count > 20 { playgroundLog.removeLast() }
        }
        return result.expansion
    }

    func playgroundReset() {
        try? core?.playgroundReset()
        playgroundBuffer = ""
    }

    func clearPlaygroundLog() { playgroundLog.removeAll() }

    // MARK: Import / Export

    func beginImport(_ url: URL) {
        guard let core else { return }
        let scoped = url.startAccessingSecurityScopedResource()
        let path = url.path
        Task {
            // Parsing (up to 10 MB) runs off the main actor; the core does not lock for it.
            let result = await Task.detached { Result { try core.parseImport(path: path) } }.value
            if scoped { url.stopAccessingSecurityScopedResource() }
            switch result {
            case .success(let parsed):
                pendingImport = PendingImport(
                    fileName: url.lastPathComponent, library: parsed.library, summary: parsed.summary)
            case .failure(let error):
                errorMessage = "Import failed: \(error.localizedDescription)"
            }
        }
    }

    func confirmImport(_ pending: PendingImport, replace: Bool) {
        guard let core else { return }
        commitNow()
        let before = library
        do {
            if replace {
                _ = try core.replace(pending.library)
                library = pending.library
                statusMessage = "Library replaced with \(pending.summary.snippets) snippets. Use Undo to restore."
            } else {
                let merged = try core.merge(pending.library)
                library = merged.library
                statusMessage = "Imported \(merged.merged.groups) groups and \(merged.merged.snippets) snippets."
            }
            lastCommitted = library
            refreshFromCore()
            registerUndo(replace ? "Replace Library" : "Import", restoring: before)
            sidebarSelection = .all
            ensureSelectionVisible()
        } catch {
            errorMessage = "Import failed: \(error.localizedDescription)"
        }
        pendingImport = nil
    }

    func exportLibrary() {
        guard commitNow(), let core else { return }
        let panel = NSSavePanel()
        panel.title = "Export Library"
        panel.allowedContentTypes = [.json]
        panel.nameFieldStringValue = "typelune-export.json"
        panel.canCreateDirectories = true
        let handle: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            do {
                let result = try core.export(path: url.path)
                self?.statusMessage = "Exported \(result.snippets) snippets to \(url.lastPathComponent)."
            } catch {
                self?.errorMessage = "Export failed: \(error.localizedDescription)"
            }
        }
        if let window = NSApp.keyWindow {
            panel.beginSheetModal(for: window, completionHandler: handle)
        } else {
            handle(panel.runModal())
        }
    }

    // MARK: System-wide expansion

    func refreshNative() {
        guard let core, !nativeBusy else { return }
        native = try? core.native(.status)
        if native?.state == .running || native?.state == .paused, Date.now.timeIntervalSince(layoutRefreshedAt) > 2 {
            try? core.refreshKeyboardLayout()  // main actor == main thread (TIS contract)
            layoutRefreshedAt = .now
        }
    }

    func nativeCommand(_ command: TypeluneCore.NativeCommand) {
        guard let core, !nativeBusy else { return }
        switch command {
        case .stop:
            // Rust signals shutdown immediately, then joins the tap/injector threads without
            // holding its state lock; run that wait off the main actor so the UI stays live.
            nativeBusy = true
            if var shown = native { shown.state = .stopping; native = shown }
            Task {
                let result = await Task.detached { Result { try core.native(.stop) } }.value
                nativeBusy = false
                switch result {
                case .success(let status): native = status
                case .failure(let error):
                    errorMessage = error.localizedDescription
                    native = try? core.native(.status)
                }
            }
        default:
            // Start (and keyboard-layout refresh) must run on the main thread: TIS contract.
            if command == .start { commitNow() }
            do {
                native = try core.native(command)
            } catch {
                errorMessage = error.localizedDescription
                native = try? core.native(.status)
            }
        }
    }

    func openAccessibilitySettings() {
        do { try core?.openAccessibilitySettings() } catch { errorMessage = error.localizedDescription }
    }

    func revealLibrary() {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: libraryPath)])
    }

    func refreshSaveStatus() {
        saveStatus = try? core?.saveStatus()
    }

    /// Called at quit: commit, then stop native expansion and flush saves inside Rust.
    func shutdown() {
        commitNow()
        core?.close()
    }

    // MARK: Private

    private func refreshFromCore() {
        guard let state = try? core?.state() else { return }
        activeRules = state.activeRules
        saveStatus = state.save
        applyLints(state.lints)
    }

    private func applyLints(_ lints: [Lint]) {
        var map: [String: [String]] = [:]
        for l in lints { map[l.snippetId, default: []].append(l.message) }
        lintsBySnippet = map
    }

    private func rebuildIndex() {
        var idx: [String: (group: Int, snippet: Int)] = [:]
        for (gi, g) in library.groups.enumerated() {
            for (si, s) in g.snippets.enumerated() { idx[s.id] = (gi, si) }
        }
        index = idx
    }
}

extension String {
    func ifEmpty(_ fallback: String) -> String { isEmpty ? fallback : self }
}
