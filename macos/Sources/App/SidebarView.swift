import SwiftUI

struct SidebarView: View {
    @Environment(AppModel.self) private var model
    @State private var renaming: SnippetGroup?
    @State private var renameText = ""
    @State private var deleting: SnippetGroup?

    var body: some View {
        @Bindable var model = model
        List(selection: $model.sidebarSelection) {
            Section("Library") {
                Label("All Snippets", systemImage: "tray.full")
                    .badge(model.library.snippetCount)
                    .tag(SidebarItem.all)
            }
            Section("Groups") {
                ForEach(model.groups) { group in
                    GroupRow(group: group)
                        .tag(SidebarItem.group(group.id))
                        .contextMenu {
                            Button(group.enabled ? "Disable Group" : "Enable Group") {
                                model.setGroupEnabled(group.id, !group.enabled)
                            }
                            Button("Rename…") {
                                renameText = group.name
                                renaming = group
                            }
                            Divider()
                            Button("Delete Group…", role: .destructive) { deleting = group }
                        }
                }
            }
        }
        .listStyle(.sidebar)
        .navigationTitle("Typelune")
        .overlay {
            if model.groups.isEmpty {
                ContentUnavailableView {
                    Label("No Groups", systemImage: "folder")
                } actions: {
                    Button("New Group") { model.newGroup() }
                }
            }
        }
        .alert(
            "Rename Group", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } }),
            presenting: renaming
        ) { group in
            TextField("Name", text: $renameText)
            Button("Rename") { model.renameGroup(group.id, to: renameText) }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog(
            "Delete “\(deleting?.name ?? "")”?",
            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            presenting: deleting
        ) { group in
            Button("Delete Group and \(group.snippets.count) Snippets", role: .destructive) {
                model.deleteGroup(group.id)
            }
        } message: { _ in
            Text("You can undo this with Edit ▸ Undo.")
        }
    }
}

private struct GroupRow: View {
    let group: SnippetGroup

    var body: some View {
        Label {
            Text(group.name)
                .foregroundStyle(group.enabled ? .primary : .secondary)
        } icon: {
            Image(systemName: group.enabled ? "folder" : "folder.badge.minus")
        }
        .badge(group.snippets.count)
        .accessibilityValue(group.enabled ? "Enabled" : "Disabled")
    }
}
