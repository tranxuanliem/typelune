import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// Apps in which Typelune never records typing, expands abbreviations or inserts Quick
/// Search text. Stored by bundle identifier, so renaming or moving an app keeps it excluded.
struct ExclusionSettings: View {
    @Environment(AppModel.self) private var model
    @State private var selection: Set<String> = []
    @State private var showPicker = false

    private var apps: [ExcludedApp] { model.preferences?.preferences.excludedApps ?? [] }

    var body: some View {
        Form {
            Section {
                List(apps, selection: $selection) { app in
                    AppRow(name: app.name, bundleID: app.bundleId)
                        .tag(app.id)
                }
                .frame(minHeight: 180)
                .overlay {
                    if apps.isEmpty {
                        Text("No excluded apps").foregroundStyle(.secondary)
                    }
                }
                .onDeleteCommand(perform: removeSelected)
                HStack {
                    Button("Add App…", systemImage: "plus") { showPicker = true }
                    Button("Remove", systemImage: "minus", action: removeSelected)
                        .disabled(selection.isEmpty)
                    Spacer()
                }
                .disabled(model.preferences?.writable != true)
            } header: {
                Text("Excluded Apps")
            } footer: {
                Text(
                    "In these apps Typelune does not record what you type, does not expand abbreviations and never inserts Quick Search text (Copy still works). This is checked when you type and again right before any text is inserted. Typelune also pauses while macOS Secure Keyboard Entry is on, but it cannot recognise every password field, so exclude apps where that matters."
                )
                .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $showPicker) {
            AppPicker(excluded: Set(apps.map(\.id))) { picked in
                showPicker = false
                add(picked)
            }
        }
    }

    private func add(_ picked: [ExcludedApp]) {
        guard !picked.isEmpty else { return }
        model.updatePreferences { prefs in
            for app in picked where !prefs.excludedApps.contains(where: { $0.id == app.id }) {
                prefs.excludedApps.append(app)
            }
            prefs.excludedApps.sort { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        }
    }

    private func removeSelected() {
        let doomed = selection
        model.updatePreferences { $0.excludedApps.removeAll { doomed.contains($0.id) } }
        selection = []
    }
}

private struct AppRow: View {
    let name: String
    let bundleID: String

    var body: some View {
        HStack {
            Image(nsImage: icon)
                .resizable()
                .frame(width: 20, height: 20)
                .accessibilityHidden(true)
            VStack(alignment: .leading) {
                Text(name.isEmpty ? bundleID : name)
                Text(bundleID).font(.caption.monospaced()).foregroundStyle(.secondary)
            }
        }
    }

    private var icon: NSImage {
        if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) {
            return NSWorkspace.shared.icon(forFile: url.path)
        }
        return NSWorkspace.shared.icon(for: .applicationBundle)
    }
}

/// Installed and running apps, searchable, plus "Choose Other…" for anything else.
private struct AppPicker: View {
    let excluded: Set<String>
    let done: ([ExcludedApp]) -> Void
    @State private var candidates: [ExcludedApp] = []
    @State private var loading = true
    @State private var query = ""
    @State private var selection: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Exclude Apps").font(.headline)
            TextField("Search", text: $query).textFieldStyle(.roundedBorder)
            List(filtered, selection: $selection) { app in
                AppRow(name: app.name, bundleID: app.bundleId).tag(app.id)
            }
            .overlay {
                if loading { ProgressView() }
            }
            HStack {
                Button("Choose Other…", action: chooseOther)
                Spacer()
                Button("Cancel", role: .cancel) { done([]) }
                    .keyboardShortcut(.cancelAction)
                Button("Exclude") { done(candidates.filter { selection.contains($0.id) }) }
                    .keyboardShortcut(.defaultAction)
                    .disabled(selection.isEmpty)
            }
        }
        .padding()
        .frame(width: 460, height: 460)
        .task {
            let found = await Task.detached { InstalledApps.scan() }.value
            let running = NSWorkspace.shared.runningApplications
                .filter { $0.activationPolicy == .regular }
                .compactMap { app -> ExcludedApp? in
                    guard let id = app.bundleIdentifier else { return nil }
                    return ExcludedApp(bundleId: id, name: app.localizedName ?? id)
                }
            var seen = Set<String>()
            candidates = (running + found)
                .filter { seen.insert($0.id).inserted && !excluded.contains($0.id) }
                .filter { $0.bundleId.lowercased() != Bundle.main.bundleIdentifier?.lowercased() }
                .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
            loading = false
        }
    }

    private var filtered: [ExcludedApp] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return candidates }
        return candidates.filter {
            $0.name.localizedCaseInsensitiveContains(q) || $0.bundleId.localizedCaseInsensitiveContains(q)
        }
    }

    private func chooseOther() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.applicationBundle]
        panel.allowsMultipleSelection = true
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        guard panel.runModal() == .OK else { return }
        let picked = panel.urls.compactMap(InstalledApps.app(at:))
        done(picked)
    }
}

enum InstalledApps {
    /// `.app` bundles in the standard application folders (two levels deep).
    static func scan() -> [ExcludedApp] {
        let fm = FileManager.default
        let roots = [
            "/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities",
            fm.homeDirectoryForCurrentUser.appendingPathComponent("Applications").path,
        ]
        var out: [ExcludedApp] = []
        for root in roots {
            guard let items = try? fm.contentsOfDirectory(atPath: root) else { continue }
            for item in items {
                let url = URL(fileURLWithPath: root).appendingPathComponent(item)
                if item.hasSuffix(".app") {
                    if let app = app(at: url) { out.append(app) }
                } else if (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true,
                    let inner = try? fm.contentsOfDirectory(atPath: url.path)
                {
                    for sub in inner where sub.hasSuffix(".app") {
                        if let app = app(at: url.appendingPathComponent(sub)) { out.append(app) }
                    }
                }
            }
        }
        return out
    }

    static func app(at url: URL) -> ExcludedApp? {
        guard let bundle = Bundle(url: url), let id = bundle.bundleIdentifier, !id.isEmpty else { return nil }
        let name =
            (bundle.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String)
            ?? (bundle.object(forInfoDictionaryKey: "CFBundleName") as? String)
            ?? url.deletingPathExtension().lastPathComponent
        return ExcludedApp(bundleId: id, name: name)
    }
}
