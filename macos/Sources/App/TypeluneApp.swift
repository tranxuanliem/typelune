// Typelune: native SwiftUI app on top of the Rust core (libtypelune_core).

import AppKit
import SwiftUI

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        MainActor.assumeIsolated {
            AppModel.live.didFinishLaunching()
        }
    }

    /// Closing the window only hides Typelune: it keeps running in the menu bar (with
    /// system-wide expansion and the Quick Search shortcut). Quit is explicit.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationWillTerminate(_ notification: Notification) {
        MainActor.assumeIsolated {
            AppModel.live.shutdown()
        }
    }
}

@main
struct TypeluneApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model = AppModel.live

    var body: some Scene {
        Window("Typelune", id: WindowID.main) {
            ContentView()
                .environment(model)
                .frame(minWidth: 860, minHeight: 520)
        }
        .defaultSize(width: 1180, height: 760)
        .windowToolbarStyle(.unified)
        .commands { AppCommands(model: model) }

        Window("Playground", id: WindowID.playground) {
            PlaygroundView()
                .environment(model)
                .frame(minWidth: 480, minHeight: 360)
        }
        .defaultSize(width: 640, height: 520)

        Window("System-wide Expansion", id: WindowID.systemWide) {
            SystemWideView()
                .environment(model)
                .frame(minWidth: 460, minHeight: 480)
        }
        .defaultSize(width: 540, height: 700)

        Window("Backups", id: WindowID.backups) {
            BackupsView()
                .environment(model)
                .frame(minWidth: 640, minHeight: 400)
        }
        .defaultSize(width: 760, height: 500)

        MenuBarExtra {
            MenuBarMenu()
                .environment(model)
        } label: {
            Image(systemName: model.menuBarSymbol)
                .accessibilityLabel("Typelune, system-wide expansion \(model.native?.state.title ?? "Off")")
        }
        .menuBarExtraStyle(.menu)

        Settings {
            SettingsView()
                .environment(model)
        }
    }
}

enum WindowID {
    static let main = "main"
    static let playground = "playground"
    static let systemWide = "system-wide"
    static let backups = "backups"
}

struct AppCommands: Commands {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("New Snippet") { model.newSnippet() }
                .keyboardShortcut("n")
            Button("New Group") { model.newGroup() }
                .keyboardShortcut("n", modifiers: [.command, .shift])
        }
        CommandGroup(replacing: .importExport) {
            Button("Import Library…") { model.showImporter = true }
                .keyboardShortcut("i", modifiers: [.command, .shift])
            Button("Export Library…") { model.exportLibrary() }
                .keyboardShortcut("e", modifiers: [.command, .shift])
            Divider()
            Button("Backups…") { openWindow(id: WindowID.backups) }
        }
        CommandMenu("Snippet") {
            let selected = model.snippetSelection
            Button("Duplicate") { selected.map(model.duplicate) }
                .keyboardShortcut("d")
                .disabled(selected == nil)
            Button(selected.flatMap(model.snippet)?.enabled == false ? "Enable" : "Disable") {
                selected.map(model.toggleEnabled)
            }
            .keyboardShortcut("e", modifiers: [.command, .option])
            .disabled(selected == nil)
            Divider()
            // No ⌘⌫ shortcut: it would steal "delete to line start" from text fields.
            // The Delete key in the snippet list (onDeleteCommand) deletes the selection.
            Button("Delete Snippet") { selected.map(model.delete) }
                .disabled(selected == nil)
        }
        CommandMenu("Expansion") {
            Button("Playground") { openWindow(id: WindowID.playground) }
                .keyboardShortcut("p", modifiers: [.command, .option])
            Button("System-wide Expansion…") { openWindow(id: WindowID.systemWide) }
                .keyboardShortcut("s", modifiers: [.command, .option])
            Button("Quick Search") { model.openQuickSearch() }
        }
    }
}
