// Thin, typed wrapper over the C ABI in include/typelune_core.h.
// One JSON request in, one JSON reply out; every returned C string is freed here.

import Foundation

struct CoreError: Error, LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

private struct Reply<T: Decodable>: Decodable {
    let ok: Bool
    let result: T?
    let error: String?
}

/// Readers/writer lock used as a lifetime barrier for the core handle.
final class LifetimeLock: @unchecked Sendable {
    private let rw: UnsafeMutablePointer<pthread_rwlock_t>

    init() {
        rw = .allocate(capacity: 1)
        pthread_rwlock_init(rw, nil)
    }

    deinit {
        pthread_rwlock_destroy(rw)
        rw.deallocate()
    }

    func read<T>(_ body: () throws -> T) rethrows -> T {
        pthread_rwlock_rdlock(rw)
        defer { pthread_rwlock_unlock(rw) }
        return try body()
    }

    func write<T>(_ body: () throws -> T) rethrows -> T {
        pthread_rwlock_wrlock(rw)
        defer { pthread_rwlock_unlock(rw) }
        return try body()
    }
}

/// Owns one `TplCore*`.
///
/// Lifetime: every FFI call runs while holding the *read* side of `lifetime`, for the whole
/// duration of `tpl_core_call` (calls may overlap; the Rust core locks its own state, and
/// import parsing deliberately runs without that state lock). `close()` takes the *write*
/// side, so the Rust box is freed only when no call is in flight, and later calls see a nil
/// handle and throw instead of touching freed memory.
///
/// Threading: `native_start` / `native_refresh_layout` must be sent on the main thread (the
/// core verifies this). `close()` is idempotent and also runs on deinit.
final class TypeluneCore: @unchecked Sendable {
    private var handle: OpaquePointer?  // guarded by `lifetime`
    private let lifetime = LifetimeLock()

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        return e
    }()

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()

    static var abiVersion: UInt32 { tpl_abi_version() }

    /// Opens the library. `dataDirectory == nil` uses the default Application Support
    /// folder (or `TYPELUNE_DATA_DIR`) and copies a pre-0.3 library once. Never starts
    /// system-wide input monitoring.
    init(dataDirectory: URL? = nil) throws {
        let ptr: OpaquePointer?
        if let dataDirectory {
            ptr = dataDirectory.path.withCString { tpl_core_open($0) }
        } else {
            ptr = tpl_core_open(nil)
        }
        guard let ptr else { throw CoreError(message: "The Typelune core could not be opened.") }
        handle = ptr
    }

    deinit { close() }

    /// Stops system-wide expansion, flushes pending saves and frees the core.
    func close() {
        lifetime.write {
            if let h = handle {
                handle = nil
                tpl_core_close(h)
            }
        }
    }

    var isOpen: Bool { lifetime.read { handle != nil } }

    func call<Request: Encodable, Result: Decodable>(_ request: Request, as: Result.Type = Result.self) throws -> Result
    {
        let data = try Self.encoder.encode(request)
        guard let json = String(data: data, encoding: .utf8) else {
            throw CoreError(message: "Request could not be encoded.")
        }
        let replyData: Data = try lifetime.read {
            guard let h = handle else { throw CoreError(message: "The Typelune core is closed.") }
            guard let raw = json.withCString({ tpl_core_call(h, $0) }) else {
                throw CoreError(message: "The Typelune core returned no reply.")
            }
            defer { tpl_string_free(raw) }
            return Data(bytes: raw, count: strlen(raw))
        }
        let reply = try Self.decoder.decode(Reply<Result>.self, from: replyData)
        if reply.ok, let result = reply.result { return result }
        throw CoreError(message: reply.error ?? "Unknown error from the Typelune core.")
    }

    // MARK: Requests

    private struct Command: Encodable { let cmd: String }
    private struct LibraryCommand: Encodable { let cmd: String; let library: Library }
    private struct PathCommand: Encodable { let cmd: String; let path: String }
    private struct BodyCommand: Encodable { let cmd: String; let body: String }
    private struct EditCommand: Encodable { let cmd = "playground_edit"; let backspaces: Int; let inserted: String }
    private struct FlushCommand: Encodable { let cmd = "flush"; let timeoutMs: Int }

    func state() throws -> CoreState { try call(Command(cmd: "state")) }
    func update(_ library: Library) throws -> UpdateResult {
        try call(LibraryCommand(cmd: "update_library", library: library))
    }
    func replace(_ library: Library) throws -> UpdateResult {
        try call(LibraryCommand(cmd: "replace_library", library: library))
    }
    func merge(_ library: Library) throws -> MergeResult {
        try call(LibraryCommand(cmd: "merge_import", library: library))
    }
    func parseImport(path: String) throws -> ParsedImport { try call(PathCommand(cmd: "parse_import", path: path)) }
    func export(path: String) throws -> ExportResult { try call(PathCommand(cmd: "export", path: path)) }
    func render(_ body: String) throws -> RenderResult { try call(BodyCommand(cmd: "render", body: body)) }
    func playgroundReset() throws { _ = try call(Command(cmd: "playground_reset"), as: Empty.self) }
    func playgroundEdit(backspaces: Int, inserted: String) throws -> PlaygroundResult {
        try call(EditCommand(backspaces: backspaces, inserted: inserted))
    }
    func flush(timeoutMs: Int) throws -> FlushResult { try call(FlushCommand(timeoutMs: timeoutMs)) }
    func saveStatus() throws -> SaveStatus { try call(Command(cmd: "save_status")) }
    func native(_ command: NativeCommand) throws -> NativeStatus { try call(Command(cmd: command.rawValue)) }
    func refreshKeyboardLayout() throws { _ = try call(Command(cmd: "native_refresh_layout"), as: Empty.self) }
    func openAccessibilitySettings() throws {
        _ = try call(Command(cmd: "open_accessibility_settings"), as: Empty.self)
    }

    enum NativeCommand: String {
        case status = "native_status"
        case start = "native_start"
        case pause = "native_pause"
        case resume = "native_resume"
        case stop = "native_stop"
    }
}
