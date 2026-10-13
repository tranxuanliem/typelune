import AppKit
import ServiceManagement
import SwiftUI

struct GeneralSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            Section {
                Text(
                    "Closing the Typelune window keeps it running in the menu bar, so system-wide expansion and the Quick Search shortcut keep working. Choose Quit Typelune (⌘Q) to stop everything."
                )
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            }
            loginSection
            startSection
            ShortcutSection()
            if let error = model.preferences?.error {
                Section { Text(error).foregroundStyle(.red) }
            }
        }
        .formStyle(.grouped)
        .onAppear {
            model.refreshLoginItem()
            model.loadPreferences()
        }
        // Approval happens in System Settings: re-read the status when Typelune is active again.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            model.refreshLoginItem()
        }
    }

    private var loginSection: some View {
        Section {
            Toggle(
                "Open Typelune at login",
                isOn: Binding(
                    get: { model.loginItemStatus == .enabled || model.loginItemStatus == .requiresApproval },
                    set: { model.setLaunchAtLogin($0) })
            )
            .disabled(!LaunchPolicy.loginItemsAllowed(isolated: model.isolatedDataFolder))
            if model.loginItemStatus == .requiresApproval {
                HStack {
                    Text("macOS needs your approval in System Settings › General › Login Items.")
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("Open Login Items…") { model.openLoginItemsSettings() }
                }
            }
            if let error = model.loginItemError {
                Text(error).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
        } footer: {
            Text(
                model.isolatedDataFolder
                    ? "Unavailable while Typelune uses a separate data folder (TYPELUNE_DATA_DIR), so test runs never register the real app."
                    : "Off unless you turn it on. Status: \(loginStatusText)."
            )
            .foregroundStyle(.secondary)
        }
    }

    private var loginStatusText: String {
        switch model.loginItemStatus {
        case .enabled: "enabled"
        case .requiresApproval: "waiting for approval"
        case .notRegistered: "not registered"
        case .notFound: "not found (move Typelune to Applications and try again)"
        @unknown default: "unknown"
        }
    }

    private var startSection: some View {
        Section {
            Toggle(
                "Start system-wide expansion when Typelune opens",
                isOn: Binding(
                    get: { model.preferences?.preferences.startExpansionAtLaunch ?? false },
                    set: { on in model.updatePreferences { $0.startExpansionAtLaunch = on } })
            )
            .disabled(model.preferences?.writable != true)
            if let notice = model.launchNotice {
                Text(notice).font(.callout).foregroundStyle(.secondary)
            }
        } footer: {
            Text(
                "Off by default. It only starts if Accessibility permission is already granted; Typelune never asks for or changes permissions."
            )
            .foregroundStyle(.secondary)
        }
    }
}

private struct ShortcutSection: View {
    @Environment(AppModel.self) private var model
    @State private var recording = false
    @State private var monitor: Any?
    @State private var problem: String?

    var body: some View {
        Section {
            // An explicit row (not LabeledContent), so the value and each button stay separate,
            // operable accessibility elements.
            HStack {
                Text("Quick Search shortcut")
                Spacer()
                Text(recording ? "Type a shortcut…" : currentText)
                    .font(.body.monospaced())
                    .foregroundStyle(recording ? .secondary : .primary)
                    .accessibilityLabel("Current Quick Search shortcut")
                    .accessibilityValue(recording ? "Recording" : currentText)
                Button(recording ? "Cancel" : "Record…") {
                    recording ? stopRecording(restore: true) : startRecording()
                }
                .accessibilityLabel(recording ? "Cancel recording" : "Record Quick Search shortcut")
                Button("Clear") { model.setHotkey(nil) }
                    .disabled(recording || model.preferences?.preferences.quickSearchHotkey == nil)
                    .accessibilityLabel("Clear Quick Search shortcut")
                Button("Default") { model.setHotkey(defaultHotkey) }
                    .disabled(recording)
                    .accessibilityLabel("Reset Quick Search shortcut to default")
            }
            .accessibilityElement(children: .contain)
            if let problem {
                Text(problem).foregroundStyle(.red)
            }
            switch model.hotkeyStatus {
            case .failed(let message): Text(message).foregroundStyle(.red)
            case .registered: Text("Active in every app.").foregroundStyle(.secondary)
            case .none: EmptyView()
            }
        } footer: {
            Text(
                "Opens a small search panel from any app. Return types the snippet into the field you came from (system-wide expansion must be running); ⌘Return copies it. Conflicts with macOS system shortcuts cannot always be detected."
            )
            .foregroundStyle(.secondary)
        }
        .onDisappear { if recording { stopRecording(restore: true) } }
    }

    private var defaultHotkey: Hotkey {
        Hotkey(keyCode: 17, command: true, option: true, control: true, shift: false, keyLabel: "T")
    }

    private var currentText: String {
        model.preferences?.preferences.quickSearchHotkey.map(HotkeyRules.display) ?? "None"
    }

    private func startRecording() {
        problem = nil
        recording = true
        model.suspendHotkey()
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            MainActor.assumeIsolated { record(event) }
            return nil
        }
    }

    private func record(_ event: NSEvent) {
        if event.keyCode == 53, event.modifierFlags.intersection(.deviceIndependentFlagsMask).isEmpty {
            stopRecording(restore: true)
            return
        }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let label = event.charactersIgnoringModifiers?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let hotkey = Hotkey(
            keyCode: Int(event.keyCode), command: flags.contains(.command), option: flags.contains(.option),
            control: flags.contains(.control), shift: flags.contains(.shift),
            keyLabel: label.isEmpty ? keyName(event.keyCode) : String(label.prefix(8)))
        if let issue = HotkeyRules.problem(hotkey) {
            problem = issue
            return
        }
        stopRecording(restore: false)
        model.setHotkey(hotkey)
    }

    private func keyName(_ code: UInt16) -> String {
        switch code {
        case 49: "Space"
        case 36: "Return"
        case 48: "Tab"
        case 51: "Delete"
        default: "Key \(code)"
        }
    }

    private func stopRecording(restore: Bool) {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        recording = false
        if restore { model.applyHotkey() }
    }
}
