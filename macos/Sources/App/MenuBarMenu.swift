import AppKit
import SwiftUI

/// The menu bar menu. Closing every window leaves Typelune running here; Quit is explicit.
struct MenuBarMenu: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        let state = model.native?.state ?? .off
        Text("System-wide Expansion: \(state.title)")
        switch state {
        case .off, .error:
            Button("Start Expansion") { model.nativeCommand(.start) }
                .disabled(model.nativeBusy || model.native?.permissions.accessibility != true)
            if model.native?.permissions.accessibility != true {
                Text("Needs Accessibility permission")
            }
        case .running, .disabled:
            Button("Pause Expansion") { model.nativeCommand(.pause) }.disabled(model.nativeBusy)
            Button("Stop Expansion") { model.nativeCommand(.stop) }.disabled(model.nativeBusy)
        case .paused:
            Button("Resume Expansion") { model.nativeCommand(.resume) }.disabled(model.nativeBusy)
            Button("Stop Expansion") { model.nativeCommand(.stop) }.disabled(model.nativeBusy)
        case .stopping:
            Text("Stopping…")
        }
        Divider()
        Button("Quick Search\(shortcutSuffix)") { model.openQuickSearch() }
        if let notice = model.quickSearchNotice {
            Text(notice)
        }
        Button("Open Library") { show(WindowID.main) }
        Button("System-wide Expansion…") { show(WindowID.systemWide) }
        Button("Backups…") { show(WindowID.backups) }
        SettingsLink { Text("Settings…") }
            .simultaneousGesture(TapGesture().onEnded { NSApp.activate() })
        Divider()
        Button("Quit Typelune") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }

    private var shortcutSuffix: String {
        if case .registered(let shown) = model.hotkeyStatus { return "  \(shown)" }
        return ""
    }

    private func show(_ id: String) {
        openWindow(id: id)
        NSApp.activate()
    }
}
