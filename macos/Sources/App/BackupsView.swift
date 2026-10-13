import SwiftUI

/// Local snapshots of the library: list, preview and restore (with confirmation).
struct BackupsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.undoManager) private var undoManager
    @State private var selection: String?
    @State private var preview: Result<ParsedImport, Error>?
    @State private var confirmRestore: BackupInfo?

    private var backups: [BackupInfo] { model.backupList?.backups ?? [] }

    var body: some View {
        HSplitView {
            List(backups, selection: $selection) { b in
                VStack(alignment: .leading, spacing: 2) {
                    Text([b.date, b.time].compactMap { $0 }.joined(separator: " "))
                    Text(
                        "\(b.kind.title) · \(ByteCountFormatter.string(fromByteCount: Int64(b.bytes), countStyle: .file))"
                    )
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
                .tag(b.name)
            }
            .overlay {
                if backups.isEmpty {
                    ContentUnavailableView(
                        "No Backups Yet", systemImage: "clock.arrow.circlepath",
                        description: Text("A daily backup is made before the first change each day."))
                }
            }
            .frame(minWidth: 220, idealWidth: 260)
            detail
                .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
        }
        .toolbar {
            ToolbarItemGroup {
                Button("Backup Now", systemImage: "plus.circle") { model.backupNow() }
                    .help("Save a snapshot of the current library")
                Button("Reveal in Finder", systemImage: "folder") { model.revealBackups() }
            }
        }
        .navigationTitle("Backups")
        .onAppear { model.refreshBackups() }
        .onChange(of: selection) { _, name in
            preview = name.map { model.readBackup($0) }
        }
        .confirmationDialog(
            "Restore this backup?",
            isPresented: Binding(get: { confirmRestore != nil }, set: { if !$0 { confirmRestore = nil } }),
            presenting: confirmRestore
        ) { b in
            Button("Restore", role: .destructive) {
                model.restoreBackup(b.name, undoManager: undoManager)
                confirmRestore = nil
            }
            Button("Cancel", role: .cancel) { confirmRestore = nil }
        } message: { b in
            Text(
                "Your whole library will be replaced by the backup from \(b.date)\(b.time.map { " " + $0 } ?? ""). The current library is saved as a backup first, and Edit › Undo in this window restores it too."
            )
        }
    }

    @ViewBuilder
    private var detail: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let error = model.backupError ?? model.backupList?.status.lastError {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            switch preview {
            case .none:
                Text("Select a backup to preview it.").foregroundStyle(.secondary)
            case .failure(let error):
                Label(
                    "This backup cannot be used: \(error.localizedDescription)", systemImage: "xmark.octagon"
                )
                .foregroundStyle(.red)
                .fixedSize(horizontal: false, vertical: true)
            case .success(let parsed):
                Text("\(parsed.summary.snippets) snippets in \(parsed.summary.groups) groups").font(.headline)
                List(parsed.library.groups) { g in
                    Section(g.name) {
                        ForEach(g.snippets.prefix(50)) { s in
                            HStack {
                                Text(s.abbreviation).font(.body.monospaced())
                                Text(s.displayName).foregroundStyle(.secondary).lineLimit(1)
                            }
                        }
                        if g.snippets.count > 50 {
                            Text("… and \(g.snippets.count - 50) more").foregroundStyle(.secondary)
                        }
                    }
                }
                HStack {
                    Spacer()
                    Button("Restore…") { confirmRestore = backups.first { $0.name == selection } }
                        .disabled(selection == nil)
                }
            }
            Spacer(minLength: 0)
            Text(
                "Kept in \(model.backupList?.folder ?? "the backups folder"). The newest \(model.backupList?.retention ?? 30) are kept; older ones are deleted automatically. Backups stay on this Mac."
            )
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding()
    }
}
