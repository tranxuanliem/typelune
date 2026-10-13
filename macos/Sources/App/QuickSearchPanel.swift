// Quick Search panel. The app and exact text field you were typing in are captured first;
// then Typelune activates and the panel takes keyboard focus (an inactive app's panel is not
// guaranteed to receive keys). On Escape, Copy or Insert, focus goes back to that app; the
// Rust injector re-verifies the exact field before typing anything.

import AppKit
import SwiftUI

/// Keyboard keys the panel handles itself (before the search field sees them).
enum QuickSearchKey {
    case up, down, primary, copy, cancel
}

final class QuickSearchPanel: NSPanel {
    var keyHandler: ((QuickSearchKey) -> Void)?

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func sendEvent(_ event: NSEvent) {
        // While an input method is composing (marked text), its keys belong to it.
        let composing = (firstResponder as? NSTextInputClient)?.hasMarkedText() ?? false
        if event.type == .keyDown, !composing, let key = Self.key(for: event), let keyHandler {
            keyHandler(key)
            return
        }
        super.sendEvent(event)
    }

    private static func key(for event: NSEvent) -> QuickSearchKey? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        switch event.keyCode {
        case 126: return .up
        case 125: return .down
        case 53: return .cancel
        case 36, 76: return flags.contains(.command) ? .copy : .primary
        default: return nil
        }
    }
}

@MainActor
final class QuickSearchController: NSObject, NSWindowDelegate {
    private var panel: QuickSearchPanel?
    /// Set while the panel is being hidden on purpose, so losing key status or the origin
    /// app becoming active again is not treated as a click-away.
    private var closingOnPurpose = false
    /// Typelune asked to be activated and has not become active yet.
    private var activationPending = false
    private var observers: [NSObjectProtocol] = []

    var isVisible: Bool { panel?.isVisible ?? false }

    func show(model: AppModel) {
        let panel = self.panel ?? makePanel(model: model)
        self.panel = panel
        installObservers()
        panel.keyHandler = { [weak model] key in model?.quickSearchKey(key) }
        let screen = NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main
        if let visible = screen?.visibleFrame {
            let size = panel.frame.size
            panel.setFrameOrigin(
                NSPoint(x: visible.midX - size.width / 2, y: visible.maxY - size.height - visible.height * 0.18))
        }
        updateFocusState()
        // Own the keyboard: activate Typelune, then make the panel key. Activation is a
        // request (it can be refused or arrive later); `didBecomeActive` re-keys the panel.
        activationPending = !NSApp.isActive
        NSApp.activate()
        panel.makeKeyAndOrderFront(nil)
        focusSearchField()
    }

    /// Hide without treating it as a click-away.
    func hide() {
        closingOnPurpose = true
        panel?.orderOut(nil)
        activationPending = false
        closingOnPurpose = false
    }

    // MARK: Key status and activation

    func windowDidBecomeKey(_ notification: Notification) {
        focusSearchField()
        updateFocusState()
    }

    func windowDidResignKey(_ notification: Notification) {
        updateFocusState()
        guard
            QuickSearchFocusPolicy.resignKeyCancels(
                panelVisible: panel?.isVisible == true, closingOnPurpose: closingOnPurpose,
                activationPending: activationPending)
        else { return }
        AppModel.live.cancelQuickSearch(.clickAway)
    }

    func windowWillClose(_ notification: Notification) {
        guard !closingOnPurpose else { return }
        AppModel.live.cancelQuickSearch(.closeButton)
    }

    private func installObservers() {
        guard observers.isEmpty else { return }
        observers.append(
            NotificationCenter.default.addObserver(
                forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
            ) { _ in
                MainActor.assumeIsolated { AppModel.live.quickSearchController.typeluneBecameActive() }
            })
        observers.append(
            NSWorkspace.shared.notificationCenter.addObserver(
                forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
            ) { note in
                let pid = (note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?
                    .processIdentifier
                MainActor.assumeIsolated { AppModel.live.quickSearchController.appActivated(pid: pid) }
            })
    }

    private func typeluneBecameActive() {
        guard let panel, panel.isVisible, !closingOnPurpose else { return }
        // Activation may re-key another Typelune window; the open panel takes the keyboard.
        if !panel.isKeyWindow { panel.makeKeyAndOrderFront(nil) }
        activationPending = false
        focusSearchField()
        updateFocusState()
    }

    private func appActivated(pid: pid_t?) {
        guard let pid else { return }
        let own = pid == ProcessInfo.processInfo.processIdentifier
        guard
            QuickSearchFocusPolicy.otherAppActivationCancels(
                panelVisible: panel?.isVisible == true, closingOnPurpose: closingOnPurpose, activatedIsTypelune: own)
        else { return }
        AppModel.live.cancelQuickSearch(.clickAway)
    }

    /// Whether the search field can actually receive typing (Typelune active, panel key).
    private func updateFocusState() {
        AppModel.live.quickSearch.hasKeyboard = NSApp.isActive && (panel?.isKeyWindow ?? false)
    }

    // MARK: Search field focus

    /// Make the search field the panel's first responder once the panel is key. If the
    /// hosting view has not created the field yet, try once more on the next run-loop turn.
    private func focusSearchField(retry: Bool = true) {
        guard let panel, panel.isKeyWindow, let content = panel.contentView else { return }
        content.layoutSubtreeIfNeeded()
        if let field = Self.searchField(in: content) {
            if panel.firstResponder !== field.currentEditor() {
                panel.makeFirstResponder(field)
            }
        } else if retry {
            DispatchQueue.main.async { [weak self] in self?.focusSearchField(retry: false) }
        }
    }

    private static func searchField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField, field.isEditable { return field }
        for sub in view.subviews {
            if let found = searchField(in: sub) { return found }
        }
        return nil
    }

    private func makePanel(model: AppModel) -> QuickSearchPanel {
        let panel = QuickSearchPanel(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 440),
            styleMask: [.titled, .fullSizeContentView, .closable],
            backing: .buffered, defer: true)
        panel.title = "Quick Search"
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        panel.level = .floating
        panel.becomesKeyOnlyIfNeeded = false
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true
        panel.delegate = self
        panel.contentView = NSHostingView(rootView: QuickSearchView().environment(model))
        return panel
    }
}

struct QuickSearchView: View {
    @Environment(AppModel.self) private var model
    @FocusState private var searchFocused: Bool

    var body: some View {
        @Bindable var session = model.quickSearch
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search snippets", text: $session.query)
                    .textFieldStyle(.plain)
                    .font(.title3)
                    .focused($searchFocused)
                    .accessibilityLabel("Search snippets by title, abbreviation, text or tag")
            }
            .padding(12)
            if !session.hasKeyboard {
                Label(
                    "Typelune does not have keyboard focus yet. Click the search field before typing.",
                    systemImage: "keyboard.badge.exclamationmark"
                )
                .font(.caption)
                .foregroundStyle(.orange)
                .padding(.horizontal, 12)
                .padding(.bottom, 8)
            }
            Divider()
            HSplitView {
                resultList
                    .frame(minWidth: 240, idealWidth: 280)
                preview
                    .frame(minWidth: 260)
            }
            Divider()
            footer
        }
        .frame(minWidth: 560, minHeight: 360)
        .onAppear { searchFocused = true }
        // Each time the panel opens again, the search field gets focus back.
        .onChange(of: session.focusRequest) { _, _ in searchFocused = true }
        .onChange(of: session.query) { _, _ in model.quickSearchQueryChanged() }
    }

    private var resultList: some View {
        let results = model.quickSearch.results
        return ScrollViewReader { proxy in
            List(
                results,
                selection: Binding(
                    get: { model.quickSearch.selection },
                    set: { model.quickSearch.selection = $0 })
            ) { r in
                VStack(alignment: .leading, spacing: 2) {
                    HStack {
                        Text(r.title).lineLimit(1)
                        if !r.active {
                            Text("Disabled").font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                    Text(r.abbreviation.isEmpty ? r.groupName : "\(r.abbreviation) · \(r.groupName)")
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .tag(r.id)
                .id(r.id)
                .contentShape(Rectangle())
                .onTapGesture(count: 2) { model.quickSearchPrimary(r.id) }
            }
            .overlay {
                if results.isEmpty {
                    ContentUnavailableView.search(text: model.quickSearch.query)
                }
            }
            .onChange(of: model.quickSearch.selection) { _, id in
                if let id { proxy.scrollTo(id) }
            }
        }
    }

    @ViewBuilder
    private var preview: some View {
        if let id = model.quickSearch.selection, let r = model.quickSearch.results.first(where: { $0.id == id }) {
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    Text(r.title).font(.headline)
                    if !r.tags.isEmpty {
                        Text(r.tags.joined(separator: ", ")).font(.caption).foregroundStyle(.secondary)
                    }
                    Text(model.quickSearchPreview(r.body))
                        .font(.body)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(12)
            }
        } else {
            Text("No snippet selected").foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var footer: some View {
        let availability = model.quickSearch.availability
        return HStack(spacing: 10) {
            if case .copyOnly(let reason) = availability {
                Label(reason, systemImage: "doc.on.clipboard")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            } else if let target = model.quickSearch.targetName {
                Label("Inserts into \(target)", systemImage: "text.insert")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Text(availability.canInsert ? "↩ Insert   ⌘↩ Copy" : "↩ Copy")
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            Button("Copy") { model.quickSearchCopy(nil) }
                .help(
                    availability.canInsert
                        ? "Copy the expanded text (⌘↩)" : "Copy the expanded text (↩ or ⌘↩)"
                )
                .accessibilityHint(
                    availability.canInsert
                        ? "Command-Return copies; Return inserts." : "Return or Command-Return copies."
                )
                .disabled(model.quickSearch.selection == nil)
            if availability.canInsert {
                Button("Insert") { model.quickSearchPrimary(nil) }
                    .keyboardShortcut(.defaultAction)
                    .help("Type the snippet into the field you came from (↩)")
                    .disabled(model.quickSearch.selection == nil)
            }
        }
        .padding(10)
    }
}
