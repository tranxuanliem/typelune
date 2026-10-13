import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("Expansion", systemImage: "text.cursor") { ExpansionSettings() }
            Tab("Placeholders", systemImage: "curlybraces") { PlaceholderHelp() }
            Tab("Data", systemImage: "externaldrive") { DataSettings() }
        }
        .frame(width: 520, height: 440)
    }
}

private struct ExpansionSettings: View {
    @Environment(AppModel.self) private var model
    @State private var otherDelimiters = ""

    var body: some View {
        Form {
            Picker("Default trigger", selection: model.settingsBinding(\.defaultTrigger)) {
                ForEach(TriggerMode.allCases) { Text($0.title).tag($0) }
            }
            .pickerStyle(.radioGroup)

            Section("Delimiters") {
                Toggle("Space", isOn: delimiter(" "))
                Toggle("Tab", isOn: delimiter("\t"))
                Toggle("Return", isOn: delimiter("\n"))
                TextField("Other characters", text: $otherDelimiters, prompt: Text(".,;:!?"))
                    .font(.body.monospaced())
                    .onSubmit(applyOther)
                    .onChange(of: otherDelimiters) { _, _ in applyOther() }
                Text("Letters and digits are never delimiters.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section {
                Toggle("Keep the delimiter after the expansion", isOn: model.settingsBinding(\.keepDelimiter))
                Toggle("Only expand at the start of a word", isOn: model.settingsBinding(\.requireWordBoundary))
            } footer: {
                Text("Abbreviations that start with punctuation (like “;d”) are exempt from the word rule.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear {
            otherDelimiters = String(model.library.settings.delimiters.filter { !" \t\n".contains($0) })
        }
    }

    private func delimiter(_ c: Character) -> Binding<Bool> {
        Binding(
            get: { model.library.settings.delimiters.contains(c) },
            set: { on in
                var set = model.library.settings.delimiters
                if on, !set.contains(c) { set.append(c) }
                if !on { set.removeAll { $0 == c } }
                guard !set.isEmpty else { return }  // at least one delimiter
                model.settingsBinding(\.delimiters).wrappedValue = set
            })
    }

    private func applyOther() {
        var set = String(model.library.settings.delimiters.filter { " \t\n".contains($0) })
        for c in otherDelimiters where !c.isLetter && !c.isNumber && !c.isWhitespace && !set.contains(c) {
            set.append(c)
        }
        guard !set.isEmpty, set != model.library.settings.delimiters else { return }
        model.settingsBinding(\.delimiters).wrappedValue = set
    }
}

private struct PlaceholderHelp: View {
    private let rows: [(String, String)] = [
        ("{{date}}", "Today, YYYY-MM-DD"),
        ("{{time}}", "Current time, HH:MM"),
        ("{{datetime}}", "YYYY-MM-DD HH:MM"),
        ("{{date:FORMAT}}", "strftime format, e.g. %A, %d %B %Y"),
        ("{{clipboard}}", "Current plain-text clipboard"),
        ("{{cursor}}", "Where the caret lands (first one only)"),
        ("\\{{", "A literal {{"),
    ]

    var body: some View {
        Form {
            Section {
                ForEach(rows, id: \.0) { token, meaning in
                    LabeledContent {
                        Text(meaning).foregroundStyle(.secondary)
                    } label: {
                        Text(token).font(.body.monospaced())
                    }
                }
            } footer: {
                Text("Unknown or invalid placeholders stay in the text exactly as written and appear as warnings.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

private struct DataSettings: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Form {
            LabeledContent("Library") {
                Text(model.libraryPath)
                    .font(.callout.monospaced())
                    .textSelection(.enabled)
                    .lineLimit(3)
            }
            LabeledContent("Saving") {
                Text(saveText).foregroundStyle(model.saveStatus?.lastError == nil ? Color.secondary : .red)
            }
            LabeledContent("Version") {
                Text(versionText).foregroundStyle(.secondary)
            }
            HStack {
                Button("Reveal in Finder") { model.revealLibrary() }
                Button("Refresh") { model.refreshSaveStatus() }
            }
        }
        .formStyle(.grouped)
        .onAppear { model.refreshSaveStatus() }
    }

    private var versionText: String {
        let app = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        return "\(app ?? "development build") (core \(model.coreVersion))"
    }

    private var saveText: String {
        guard let s = model.saveStatus else { return "Unknown" }
        if let e = s.lastError { return e }
        if s.pending { return "Saving…" }
        if let at = s.lastSavedAt { return "Saved at \(at)" }
        return "No changes this session"
    }
}
