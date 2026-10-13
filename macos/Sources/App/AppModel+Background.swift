// Background features: menu bar status, preferences (stored next to the library), the
// global Quick Search shortcut and panel, launch at login, start-at-launch, backups and CSV
// import previews.

import AppKit
import Observation
import ServiceManagement
import SwiftUI

/// State of one Quick Search panel session.
@Observable
@MainActor
final class QuickSearchSession {
    var query = ""
    var selection: String?
    private(set) var results: [QuickSearchResult] = []
    /// The app and field captured before the panel opened.
    private(set) var target: CapturedTarget?
    private(set) var captureError: String?
    private(set) var targetName: String?
    /// The app to give focus back to, if Typelune somehow became active.
    @ObservationIgnored fileprivate(set) var originApp: NSRunningApplication?
    var availability: InsertAvailability = .copyOnly("")
    /// Bumped on every open so the search field takes focus again.
    private(set) var focusRequest = 0
    /// Typelune is active and the panel is key, so typing reaches the search field.
    var hasKeyboard = false

    fileprivate func begin(target: CapturedTarget?, error: String?, origin: NSRunningApplication?) {
        self.target = target
        captureError = error
        originApp = origin
        targetName = origin?.localizedName
        query = ""
        selection = nil
        focusRequest += 1
    }

    fileprivate func setResults(_ results: [QuickSearchResult]) {
        self.results = results
        if selection.map({ id in !results.contains { $0.id == id } }) ?? true {
            selection = results.first?.id
        }
    }

    fileprivate func end() {
        target = nil
        originApp = nil
        captureError = nil
    }
}

extension AppModel {
    // MARK: Launch

    /// Called once after launch: preferences, shortcut, frontmost-app tracking, status
    /// polling and the remembered (opt-in) start of system-wide expansion.
    func didFinishLaunching() {
        loadPreferences()
        applyHotkey()
        refreshLoginItem()
        observeFrontmostApp()
        startStatusPolling()
        refreshNative()
        guard let prefs = preferences?.preferences, prefs.startExpansionAtLaunch else { return }
        let granted = native?.permissions.accessibility ?? false
        if LaunchPolicy.shouldAutoStart(optedIn: true, accessibilityGranted: granted) {
            nativeCommand(.start)
            launchNotice =
                native?.state == .running
                ? "System-wide expansion started automatically (Settings › General)."
                : "System-wide expansion was set to start automatically but did not start: \(native?.error ?? "unknown error")."
        } else {
            launchNotice =
                "System-wide expansion was not started automatically: Accessibility permission is not granted to this build."
        }
        statusMessage = launchNotice
    }

    private func observeFrontmostApp() {
        if let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier {
            try? core?.setFrontmost(pid: pid)
        }
        workspaceObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { note in
            let pid = (note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?
                .processIdentifier
            MainActor.assumeIsolated {
                if let pid { try? AppModel.live.core?.setFrontmost(pid: pid) }
            }
        }
    }

    private func startStatusPolling() {
        statusTask?.cancel()
        statusTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                self?.refreshNative()
            }
        }
    }

    // MARK: Preferences

    func loadPreferences() {
        guard let core else { return }
        do { preferences = try core.preferences() } catch { errorMessage = error.localizedDescription }
    }

    /// Change preferences through Rust (validated, written atomically to the data folder).
    @discardableResult
    func updatePreferences(_ change: (inout Preferences) -> Void) -> Bool {
        guard let core, var prefs = preferences?.preferences else { return false }
        change(&prefs)
        do {
            preferences = try core.setPreferences(prefs)
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    var isolatedDataFolder: Bool { preferences?.isolated ?? (loadInfo?.isolated ?? true) }

    // MARK: Global shortcut

    func applyHotkey() {
        hotkeyStatus = hotKeys.register(preferences?.preferences.quickSearchHotkey) { [weak self] in
            self?.toggleQuickSearch()
        }
    }

    func setHotkey(_ hotkey: Hotkey?) {
        if updatePreferences({ $0.quickSearchHotkey = hotkey }) { applyHotkey() }
    }

    /// While recording a new shortcut, the old one must not fire.
    func suspendHotkey() {
        hotKeys.unregister()
        hotkeyStatus = .none
    }

    // MARK: Launch at login (SMAppService; never registered automatically)

    func refreshLoginItem() {
        loginItemStatus = SMAppService.mainApp.status
    }

    func setLaunchAtLogin(_ on: Bool) {
        loginItemError = nil
        guard LaunchPolicy.loginItemsAllowed(isolated: isolatedDataFolder) else {
            loginItemError = "Not available while using a separate data folder (TYPELUNE_DATA_DIR)."
            return
        }
        do {
            if on {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            loginItemError =
                "\(on ? "Could not add" : "Could not remove") the login item: \(error.localizedDescription)"
        }
        refreshLoginItem()
    }

    func openLoginItemsSettings() {
        SMAppService.openSystemSettingsLoginItems()
    }

    // MARK: Quick Search

    func toggleQuickSearch() {
        if quickSearchController.isVisible {
            cancelQuickSearch(.shortcut)
        } else {
            openQuickSearch()
        }
    }

    /// Capture where the user is typing *before* the panel takes keyboard focus.
    func openQuickSearch() {
        let committed = commitNow()
        // First, before anything else: search typing must never feed the automatic matcher,
        // even if Typelune's activation is refused and keys still reach the app below.
        _ = try? core?.setInhibited(true)
        // Captured before Typelune activates: the app and exact field to return to.
        let origin = NSWorkspace.shared.frontmostApplication
        var target: CapturedTarget?
        var captureError: String?
        if let core {
            do { target = try core.captureTarget() } catch { captureError = error.localizedDescription }
        }
        refreshNative()
        quickSearch.begin(target: target, error: captureError, origin: origin)
        quickSearch.availability = InsertAvailability.evaluate(
            native: native?.state, target: target, captureError: captureError, editsCommitted: committed)
        quickSearchQueryChanged()
        quickSearchController.show(model: self)
    }

    func quickSearchQueryChanged() {
        quickSearch.setResults(QuickSearch.results(in: library, query: quickSearch.query))
    }

    func quickSearchKey(_ key: QuickSearchKey) {
        let ids = quickSearch.results.map(\.id)
        switch key {
        case .up: quickSearch.selection = QuickSearch.move(quickSearch.selection, by: -1, in: ids)
        case .down: quickSearch.selection = QuickSearch.move(quickSearch.selection, by: 1, in: ids)
        case .primary: quickSearchPrimary(nil)
        case .copy: quickSearchCopy(nil)
        case .cancel: cancelQuickSearch(.escape)
        }
    }

    /// End the session without inserting. Focus returns to the original app only after an
    /// explicit dismissal and only if Typelune is still frontmost.
    func cancelQuickSearch(_ reason: QuickSearchDismissal) {
        let origin = quickSearch.originApp
        quickSearchController.hide()
        try? core?.releaseTarget()
        _ = try? core?.setInhibited(false)
        quickSearch.end()
        restoreOrigin(origin, after: reason)
    }

    /// Hand activation back to `origin` when the policy allows it.
    private func restoreOrigin(_ origin: NSRunningApplication?, after reason: QuickSearchDismissal) {
        guard let origin else { return }
        let me = NSRunningApplication.current
        let restore = QuickSearchFocusPolicy.restoreOrigin(
            after: reason,
            typeluneFrontmost: NSWorkspace.shared.frontmostApplication?.processIdentifier == me.processIdentifier,
            originRunning: !origin.isTerminated,
            originIsTypelune: origin.processIdentifier == me.processIdentifier)
        guard restore else { return }
        NSApp.yieldActivation(to: origin)
        origin.activate()
    }

    /// Placeholders rendered for the preview (the clipboard is read only if used).
    func quickSearchPreview(_ body: String) -> String {
        render(body)?.text ?? body
    }

    /// Return: insert when allowed, otherwise copy.
    func quickSearchPrimary(_ id: String?) {
        if let id { quickSearch.selection = id }
        if quickSearch.availability.canInsert {
            quickSearchInsert()
        } else {
            quickSearchCopy(nil)
        }
    }

    func quickSearchCopy(_ id: String?) {
        guard let id = id ?? quickSearch.selection, let snippet = snippet(id) else { return }
        let text = render(snippet.body)?.text ?? snippet.body
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
        quickSearchNotice = "Copied “\(snippet.displayName)”."
        cancelQuickSearch(.copy)
    }

    /// Hide the panel, give focus back to the original app if needed, then let Rust type
    /// the snippet through the guarded injector (it waits for the exact original field).
    private func quickSearchInsert() {
        guard let core, let id = quickSearch.selection, let target = quickSearch.target else { return }
        // Never type a body older than the one shown: pending edits must reach the core first.
        guard commitNow() else {
            quickSearch.availability = InsertAvailability.evaluate(
                native: native?.state, target: target, captureError: nil, editsCommitted: false)
            NSSound.beep()
            return
        }
        let origin = quickSearch.originApp
        quickSearchController.hide()
        quickSearch.end()
        // Give the keyboard back to the original app; the injector then waits (bounded) for
        // the exact captured field to be focused again before typing anything.
        restoreOrigin(origin, after: .insert)
        // Hand-off: automatic matching resumes; the manual insertion itself is guarded by the
        // injector (any real key or click from now on cancels it).
        _ = try? core.setInhibited(false)
        do {
            native = try core.quickInsert(token: target.token, snippetID: id)
            let before = native?.stats.lastQuickInsert
            Task { [weak self] in
                // Report the outcome; nothing is shown over the app the user is typing in.
                for _ in 0..<20 {
                    try? await Task.sleep(for: .milliseconds(150))
                    guard let self else { return }
                    self.refreshNative()
                    if let now = self.native?.stats.lastQuickInsert, now != before {
                        self.quickSearchNotice = now
                        if now.hasPrefix("Not inserted") { NSSound.beep() }
                        return
                    }
                }
            }
        } catch {
            quickSearchNotice = "Not inserted: \(error.localizedDescription)"
            NSSound.beep()
        }
    }

    // MARK: Backups

    func refreshBackups() {
        guard let core else { return }
        do {
            backupList = try core.backups()
            backupError = nil
        } catch {
            backupError = error.localizedDescription
        }
    }

    func backupNow() {
        guard commitNow(), let core else { return }
        do {
            backupList = try core.backupNow()
            backupError = nil
            statusMessage = "Backup created."
        } catch {
            backupError = "Backup failed: \(error.localizedDescription)"
        }
    }

    func readBackup(_ name: String) -> Result<ParsedImport, Error> {
        guard let core else { return .failure(CoreError(message: "The Typelune core is not available.")) }
        return Result { try core.readBackup(name) }
    }

    func revealBackups() {
        guard let folder = backupList?.folder else { return }
        let url = URL(fileURLWithPath: folder)
        if FileManager.default.fileExists(atPath: url.path) {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } else {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: libraryPath)])
        }
    }

    // MARK: Menu bar

    var menuBarSymbol: String {
        switch native?.state {
        case .running: "text.cursor"
        case .paused: "pause.circle"
        case .error, .disabled: "exclamationmark.triangle"
        default: "character.cursor.ibeam"
        }
    }
}
