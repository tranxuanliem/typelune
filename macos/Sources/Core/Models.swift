// Codable mirrors of the Rust core's JSON. Field names follow the on-disk schema
// (`atext-rs.library` v1) via snake_case conversion. Identifiers stay `String` so the
// exact UUID text written by Rust round-trips unchanged (Swift's UUID would upper-case it).

import Foundation

enum TriggerMode: String, Codable, CaseIterable, Identifiable, Sendable {
    case immediate
    case delimiter

    var id: Self { self }

    var title: String {
        switch self {
        case .immediate: "Immediately"
        case .delimiter: "After a Delimiter"
        }
    }
}

enum SnippetTrigger: String, Codable, CaseIterable, Identifiable, Sendable {
    case useDefault = "use_default"
    case immediate
    case delimiter

    var id: Self { self }

    var title: String {
        switch self {
        case .useDefault: "Library Default"
        case .immediate: "Immediately"
        case .delimiter: "After a Delimiter"
        }
    }

    func resolved(default mode: TriggerMode) -> TriggerMode {
        switch self {
        case .useDefault: mode
        case .immediate: .immediate
        case .delimiter: .delimiter
        }
    }
}

struct Snippet: Codable, Identifiable, Equatable, Sendable {
    var id: String
    var title: String
    var abbreviation: String
    var body: String
    var tags: [String]
    var enabled: Bool
    var caseSensitive: Bool
    var trigger: SnippetTrigger

    var displayName: String {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty { return title }
        let firstLine = body.split(separator: "\n", omittingEmptySubsequences: true).first.map(String.init) ?? ""
        return firstLine.isEmpty ? "Untitled" : String(firstLine.prefix(40))
    }

    func matches(_ query: String) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces)
        if q.isEmpty { return true }
        return title.localizedCaseInsensitiveContains(q)
            || abbreviation.localizedCaseInsensitiveContains(q)
            || body.localizedCaseInsensitiveContains(q)
            || tags.contains { $0.localizedCaseInsensitiveContains(q) }
    }

    /// New snippet with a fresh lowercase UUID string (same text format Rust writes).
    static func new(title: String = "New Snippet") -> Snippet {
        Snippet(
            id: UUID().uuidString.lowercased(), title: title, abbreviation: "", body: "",
            tags: [], enabled: true, caseSensitive: false, trigger: .useDefault)
    }
}

struct SnippetGroup: Codable, Identifiable, Equatable, Sendable {
    var id: String
    var name: String
    var enabled: Bool
    var snippets: [Snippet]

    static func new(name: String) -> SnippetGroup {
        SnippetGroup(id: UUID().uuidString.lowercased(), name: name, enabled: true, snippets: [])
    }
}

struct LibrarySettings: Codable, Equatable, Sendable {
    var defaultTrigger: TriggerMode
    var delimiters: String
    var keepDelimiter: Bool
    var requireWordBoundary: Bool
}

struct Library: Codable, Equatable, Sendable {
    var format: String
    var version: Int
    var settings: LibrarySettings
    var groups: [SnippetGroup]

    var snippetCount: Int { groups.reduce(0) { $0 + $1.snippets.count } }

    /// Group and snippet indices for a snippet id.
    func locate(_ snippetID: String) -> (group: Int, snippet: Int)? {
        for (gi, g) in groups.enumerated() {
            if let si = g.snippets.firstIndex(where: { $0.id == snippetID }) { return (gi, si) }
        }
        return nil
    }
}

struct Lint: Codable, Equatable, Sendable {
    var snippetId: String
    var message: String
}

struct SaveStatus: Codable, Equatable, Sendable {
    var saves: Int
    var coalesced: Int
    var lastError: String?
    var lastSavedAt: String?
    var pending: Bool
}

struct MigrationInfo: Codable, Equatable, Sendable {
    /// "copied", "not_needed" or "no_legacy".
    var kind: String
    var bytes: Int?
    var sha256: String?
}

struct LoadInfo: Codable, Equatable, Sendable {
    /// "loaded", "created_demo", "recovered", "memory_only" or "migration_failed".
    var kind: String
    var backup: String?
    var error: String?
    var migration: MigrationInfo?
    var deprecatedEnv: Bool?
}

struct CoreState: Codable, Sendable {
    var library: Library
    var libraryPath: String
    var load: LoadInfo
    var lints: [Lint]
    var activeRules: Int
    var save: SaveStatus
    var abiVersion: Int
    var coreVersion: String
}

struct UpdateResult: Codable, Sendable {
    var lints: [Lint]
    var activeRules: Int
    var save: SaveStatus
}

struct MergeSummary: Codable, Sendable {
    var groups: Int
    var snippets: Int
}

struct MergeResult: Codable, Sendable {
    var lints: [Lint]
    var activeRules: Int
    var save: SaveStatus
    var merged: MergeSummary
    var library: Library
}

struct ParsedImport: Codable, Sendable {
    var library: Library
    var summary: MergeSummary
}

struct RenderResult: Codable, Equatable, Sendable {
    var text: String
    var hasCursor: Bool
    var beforeCursor: String
    var afterCursor: String
    var warnings: [String]
}

struct PlaygroundExpansion: Codable, Equatable, Sendable {
    var title: String
    var consumedChars: Int
    var deleteText: String
    var insert: String
    var afterCursor: String
    var warnings: [String]
}

struct PlaygroundResult: Codable, Sendable {
    var expansion: PlaygroundExpansion?
    var buffer: String
}

struct ExportResult: Codable, Sendable {
    var snippets: Int
}

struct FlushResult: Codable, Sendable {
    var flushed: Bool
    var save: SaveStatus
}

struct Empty: Codable, Sendable {}

struct Permissions: Codable, Equatable, Sendable {
    var supported: Bool
    var accessibility: Bool
    var listenEvents: Bool
    var postEvents: Bool
    var secureInput: Bool
}

struct NativeDiagnostics: Codable, Equatable, Sendable {
    var keyEventsSeen: Int
    var textInputs: Int
    var emptyTextEvents: Int
    var resetInputs: Int
    var backspaces: Int
    var mouseResets: Int
    var ownEventSkips: Int
    var pausedSkips: Int
    var typedDuringInjection: Int
    var targetPidPresent: Int
    var layoutTranslated: Int
    var ownAppSkips: Int
    var matchesQueued: Int
    var mouseDuringInjection: Int
    var lastCancelEvent: String?
}

struct NativeStats: Codable, Equatable, Sendable {
    var expansions: Int
    var lastSnippet: String?
    var lastAt: String?
    var secureInputSkips: Int
    var tapReenabled: Int
    var lastWarning: String?
    var aborted: Int
    var lastAbort: String?
    var diag: NativeDiagnostics
}

enum NativeRunState: String, Codable, Sendable {
    case off, running, paused, disabled, stopping, error

    var title: String {
        switch self {
        case .off: "Off"
        case .running: "Running"
        case .paused: "Paused"
        case .disabled: "Disabled by macOS"
        case .stopping: "Stopping…"
        case .error: "Error"
        }
    }
}

struct NativeStatus: Codable, Equatable, Sendable {
    var state: NativeRunState
    var error: String?
    var permissions: Permissions
    var processPath: String
    var stats: NativeStats
}
