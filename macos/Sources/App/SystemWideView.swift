import SwiftUI

struct SystemWideView: View {
    @Environment(AppModel.self) private var model
    @State private var showDiagnostics = false

    var body: some View {
        Form {
            Section {
                Text(
                    "When started, Typelune watches key presses in other apps through a macOS event tap and replaces abbreviations. It is off when the app launches unless you turned on “Start system-wide expansion when Typelune opens” in Settings › General, and it keeps running after you close the window until you press Stop or quit Typelune."
                )
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            }
            statusSection
            permissionsSection
            statisticsSection
            Section {
                DisclosureGroup("Diagnostics", isExpanded: $showDiagnostics) {
                    DiagnosticsList(diag: model.native?.stats.diag)
                }
            } footer: {
                Text("Counts only, for this session. No typed text or key codes are recorded or saved.")
                    .foregroundStyle(.secondary)
            }
            Section("Privacy") {
                Text(
                    "Typed characters are kept only in a 64-character in-memory buffer for matching, cleared when you switch apps or open Quick Search. Expansion pauses while macOS Secure Keyboard Entry is active; not every application's password field turns that on, so not every password field is detected independently. Typing in Typelune's own windows, in Quick Search and in apps excluded in Settings is ignored. Typelune never changes permissions itself."
                )
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
        .formStyle(.grouped)
        .navigationTitle("System-wide Expansion")
        .task {
            // Poll status while this window is open (cheap, read-only queries).
            while !Task.isCancelled {
                model.refreshNative()
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    private var state: NativeRunState { model.native?.state ?? .off }
    private var permissions: Permissions? { model.native?.permissions }

    private var statusSection: some View {
        Section("Status") {
            LabeledContent("State") {
                Label(state.title, systemImage: stateSymbol)
                    .foregroundStyle(state == .running ? .green : (state == .error ? .red : .secondary))
            }
            if let error = model.native?.error {
                Text(error)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                switch state {
                case .off, .error:
                    Button("Start", systemImage: "play.fill") { model.nativeCommand(.start) }
                        .buttonStyle(.borderedProminent)
                        .disabled(!(permissions?.accessibility ?? false))
                        .help(
                            permissions?.accessibility == true
                                ? "Start system-wide expansion"
                                : "Requires Accessibility permission, which you grant in System Settings.")
                case .running, .disabled:
                    Button("Pause", systemImage: "pause.fill") { model.nativeCommand(.pause) }
                    Button("Stop", systemImage: "stop.fill") { model.nativeCommand(.stop) }
                case .paused:
                    Button("Resume", systemImage: "play.fill") { model.nativeCommand(.resume) }
                    Button("Stop", systemImage: "stop.fill") { model.nativeCommand(.stop) }
                case .stopping:
                    ProgressView().controlSize(.small)
                    Text("Stopping…").foregroundStyle(.secondary)
                }
                Spacer()
            }
            .disabled(model.nativeBusy)
        }
    }

    private var stateSymbol: String {
        switch state {
        case .running: "bolt.fill"
        case .paused: "pause.circle"
        case .disabled, .error: "exclamationmark.triangle"
        case .off: "bolt.slash"
        case .stopping: "hourglass"
        }
    }

    private var permissionsSection: some View {
        Section {
            PermissionRow(title: "Accessibility", detail: "Required", granted: permissions?.accessibility)
            PermissionRow(title: "Event Posting", detail: "Preflight", granted: permissions?.postEvents)
            PermissionRow(title: "Input Monitoring", detail: "Informational", granted: permissions?.listenEvents)
            LabeledContent("Secure Keyboard Entry") {
                Text(permissions?.secureInput == true ? "Active — typing not observed" : "Inactive")
                    .foregroundStyle(.secondary)
            }
            if permissions?.accessibility == false {
                Button("Open Accessibility Settings…") { model.openAccessibilitySettings() }
            }
        } header: {
            Text("Permissions")
        } footer: {
            Text(
                "Permission belongs to \(model.native?.processPath ?? "this app"). The app is ad-hoc signed, so after an update macOS may need it removed and added again."
            )
            .foregroundStyle(.secondary)
        }
    }

    private var statisticsSection: some View {
        Section("This Session") {
            let stats = model.native?.stats
            LabeledContent("Expansions", value: "\(stats?.expansions ?? 0)")
            LabeledContent(
                "Last Snippet",
                value: [stats?.lastSnippet, stats?.lastAt].compactMap { $0 }.joined(separator: " at ").ifEmpty("—"))
            LabeledContent("Aborted", value: "\(stats?.aborted ?? 0)")
            if let reason = stats?.lastAbort {
                LabeledContent("Last Abort", value: reason)
            }
            if let warning = stats?.lastWarning {
                LabeledContent("Last Warning", value: warning)
            }
            if let quick = stats?.lastQuickInsert {
                LabeledContent("Last Quick Search", value: quick)
            }
            LabeledContent("Secure-input Skips", value: "\(stats?.secureInputSkips ?? 0)")
            LabeledContent("Tap Re-enabled", value: "\(stats?.tapReenabled ?? 0)")
        }
    }
}

private struct PermissionRow: View {
    let title: String
    let detail: String
    let granted: Bool?

    var body: some View {
        LabeledContent {
            if let granted {
                Label(
                    granted ? "Granted" : "Not Granted", systemImage: granted ? "checkmark.circle.fill" : "xmark.circle"
                )
                .foregroundStyle(granted ? .green : .secondary)
            } else {
                Text("Unknown").foregroundStyle(.secondary)
            }
        } label: {
            Text(title)
            Text(detail)
        }
    }
}

private struct DiagnosticsList: View {
    let diag: NativeDiagnostics?

    var body: some View {
        if let d = diag {
            Group {
                row("Key-down events seen", d.keyEventsSeen)
                row("Usable text inputs", d.textInputs)
                row("Empty-Unicode key events", d.emptyTextEvents)
                row("Recovered via keyboard layout", d.layoutTranslated)
                row("Resets (shortcut, navigation)", d.resetInputs)
                row("Backspaces", d.backspaces)
                row("Own synthetic events skipped", d.ownEventSkips)
                row("Typed into Typelune (ignored)", d.ownAppSkips)
            }
            Group {
                row("Skipped while paused", d.pausedSkips)
                row("Typing that cancelled an expansion", d.typedDuringInjection)
                row("Events with a target PID", d.targetPidPresent)
                row("Mouse clicks (reset)", d.mouseResets)
                row("Clicks that cancelled an expansion", d.mouseDuringInjection)
                row("Matches queued", d.matchesQueued)
            }
            Group {
                row("Keys in excluded apps (ignored)", d.excludedSkips)
                row("Keys with unknown target app (ignored)", d.unknownTargetSkips)
                row("Keys while Quick Search was open (ignored)", d.inhibitedSkips)
                row("Resets after switching apps", d.focusChangeResets)
                row("Quick Search insertions queued", d.quickInsertsQueued)
            }
            LabeledContent("Last Cancelling Event", value: d.lastCancelEvent ?? "—")
                .font(.callout)
        } else {
            Text("No data yet.").foregroundStyle(.secondary)
        }
    }

    private func row(_ title: String, _ value: Int) -> some View {
        LabeledContent(title) {
            Text(value, format: .number).monospacedDigit()
        }
    }
}
