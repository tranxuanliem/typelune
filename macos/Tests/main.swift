// Swift ⇄ Rust bridge tests. Built as a plain executable (no Xcode/XCTest needed) together
// with macos/Sources/Core. Uses isolated temp directories only; never starts system-wide
// expansion, never posts events.

import Foundation

nonisolated(unsafe) var failures = 0
nonisolated(unsafe) var passes = 0

func check(_ condition: @autoclosure () throws -> Bool, _ message: String, file: String = #fileID, line: Int = #line) {
    let result: Bool
    do { result = try condition() } catch {
        failures += 1
        print("FAIL \(file):\(line): \(message) — threw \(error)")
        return
    }
    if result {
        passes += 1
    } else {
        failures += 1
        print("FAIL \(file):\(line): \(message)")
    }
}

func test(_ name: String, _ body: () throws -> Void) {
    do {
        try body()
        print("ok   \(name)")
    } catch {
        failures += 1
        print("FAIL \(name): threw \(error)")
    }
}

func tempDir() -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("typelune-bridge-\(UUID().uuidString)")
    try! FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

func jsonObject(_ data: Data) -> NSDictionary? {
    (try? JSONSerialization.jsonObject(with: data)) as? NSDictionary
}

/// Simulates typing into a text view: feeds each character through PlaygroundMath and the
/// Rust engine, and applies expansions exactly like the Playground coordinator does.
func typeIntoPlayground(_ core: TypeluneCore, _ keys: [String], imeCommit: Bool = false) throws -> String {
    var text = ""
    try core.playgroundReset()
    for key in keys {
        let start = (text as NSString).length
        let feed = PlaygroundMath.classify(removed: "", inserted: key, endsAtCaret: true, replacesMarkedText: imeCommit)
        text += key
        guard case .typed(let backspaces, let inserted) = feed else { continue }
        let result = try core.playgroundEdit(backspaces: backspaces, inserted: inserted)
        if let exp = result.expansion,
            let rep = PlaygroundMath.replacement(in: text, editStart: start, inserted: inserted, expansion: exp)
        {
            text = (text as NSString).replacingCharacters(in: rep.range, with: rep.text)
        }
    }
    return text
}

test("abi version") {
    check(TypeluneCore.abiVersion == 2, "ABI version is 2")
}

test("open creates demo; ids keep Rust's exact text") {
    let dir = tempDir()
    let core = try TypeluneCore(dataDirectory: dir)
    defer { core.close() }
    let state = try core.state()
    check(state.load.kind == "created_demo", "fresh dir creates demo")
    check(state.library.snippetCount > 5, "demo has snippets")
    let ids = state.library.groups.flatMap { [$0.id] + $0.snippets.map(\.id) }
    check(ids.allSatisfy { $0 == $0.lowercased() }, "ids stay lowercase strings")
    check(Set(ids).count == ids.count, "ids unique (stable identity)")
}

test("Swift Codable round trip is lossless against the on-disk schema") {
    let dir = tempDir()
    let core = try TypeluneCore(dataDirectory: dir)
    defer { core.close() }
    let library = try core.state().library
    let onDisk = try Data(contentsOf: dir.appendingPathComponent("library.json"))
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    let swiftEncoded = try encoder.encode(library)
    check(jsonObject(onDisk) == jsonObject(swiftEncoded), "Swift re-encoding equals the Rust-written JSON")
}

test("loading an existing library does not rewrite it") {
    let dir = tempDir()
    try TypeluneCore(dataDirectory: dir).close()  // creates demo
    let file = dir.appendingPathComponent("library.json")
    let before = try Data(contentsOf: file)
    let mtime = try FileManager.default.attributesOfItem(atPath: file.path)[.modificationDate] as? Date
    let core = try TypeluneCore(dataDirectory: dir)
    _ = try core.state()
    _ = try core.render("x")
    core.close()
    check(try Data(contentsOf: file) == before, "bytes unchanged")
    check(
        (try FileManager.default.attributesOfItem(atPath: file.path)[.modificationDate] as? Date) == mtime,
        "mtime unchanged")
}

test("update validates, saves in background and rejects invalid libraries") {
    let dir = tempDir()
    let core = try TypeluneCore(dataDirectory: dir)
    defer { core.close() }
    var library = try core.state().library
    library.groups[0].snippets[0].body = "Xin chào 👋"
    _ = try core.update(library)
    check(try core.flush(timeoutMs: 5000).flushed, "flush completes")
    let disk = try Data(contentsOf: dir.appendingPathComponent("library.json"))
    check(String(decoding: disk, as: UTF8.self).contains("Xin chào 👋"), "edit persisted")

    var bad = library
    bad.groups[0].name = "  "
    do {
        _ = try core.update(bad)
        check(false, "invalid library must be rejected")
    } catch {
        check(error.localizedDescription.contains("name is empty"), "clear validation error")
    }
    check(try core.state().library == library, "previous library kept")
}

test("render splits at the cursor and keeps unknown tokens") {
    let core = try TypeluneCore(dataDirectory: tempDir())
    defer { core.close() }
    let r = try core.render("Chào {{cursor}}bạn {{nope}}")
    check(r.hasCursor && r.beforeCursor == "Chào " && r.afterCursor == "bạn {{nope}}", "cursor split")
    check(r.warnings.count == 1, "unknown token warned")
}

test("playground typing expands through the Rust engine") {
    let core = try TypeluneCore(dataDirectory: tempDir())
    defer { core.close() }
    check(try typeIntoPlayground(core, ["s", "a", "y", " ", "h", "l", "o", " "]) == "say Hello there! ", "hlo + space")
    check(try typeIntoPlayground(core, ["o", "h", "l", "o", " "]) == "ohlo ", "word boundary respected")
    // An input method committing decomposed text in one step.
    check(
        try typeIntoPlayground(core, ["cmo\u{31B}n "], imeCommit: true) == "Cảm ơn bạn nhiều nhé. ",
        "decomposed commit replaced fully")
    check(try typeIntoPlayground(core, ["hlo pasted "]) == "hlo pasted ", "a paste-sized insert does not expand")
    check(try typeIntoPlayground(core, ["x", "c", " "]) == "Xin chào, rất vui được gặp bạn! ", "Vietnamese body")
}

test("playground math: classification and caret placement") {
    check(
        PlaygroundMath.classify(removed: "", inserted: "a", endsAtCaret: true, replacesMarkedText: false)
            == .typed(backspaces: 0, inserted: "a"), "typing")
    check(
        PlaygroundMath.classify(removed: "a", inserted: "", endsAtCaret: true, replacesMarkedText: false)
            == .typed(backspaces: 1, inserted: ""), "backspace")
    check(
        PlaygroundMath.classify(removed: "", inserted: "pasted text", endsAtCaret: true, replacesMarkedText: false)
            == .reset, "paste resets")
    check(
        PlaygroundMath.classify(removed: "", inserted: "a", endsAtCaret: false, replacesMarkedText: false) == .reset,
        "edit away from caret resets")
    check(
        PlaygroundMath.classify(removed: "aa", inserted: "â", endsAtCaret: true, replacesMarkedText: true)
            == .typed(backspaces: 0, inserted: "â"), "IME commit not backspaced")
    check(PlaygroundMath.utf16Prefix(of: "👍🏽x", scalars: 2) == 4, "UTF-16 prefix of emoji scalars")
    let exp = PlaygroundExpansion(
        title: "t", consumedChars: 1, deleteText: "tag ", insert: "<b></b> ", afterCursor: "</b> ", warnings: [])
    let rep = PlaygroundMath.replacement(in: "x tag ", editStart: 5, inserted: " ", expansion: exp)
    check(
        rep == PlaygroundMath.Replacement(range: NSRange(location: 2, length: 4), text: "<b></b> ", caret: 5),
        "caret lands inside the tag")
    check(
        PlaygroundMath.replacement(in: "x tug ", editStart: 5, inserted: " ", expansion: exp) == nil,
        "never deletes mismatching text")
}

test("import, merge, replace and protected export") {
    let dir = tempDir()
    let core = try TypeluneCore(dataDirectory: dir)
    defer { core.close() }
    let out = dir.appendingPathComponent("export.json")
    _ = try core.export(path: out.path)
    let parsed = try core.parseImport(path: out.path)
    let merged = try core.merge(parsed.library)
    check(merged.library.snippetCount == 2 * parsed.summary.snippets, "merge doubles snippets")
    let mergedIDs = merged.library.groups.flatMap { $0.snippets.map(\.id) }
    check(Set(mergedIDs).count == mergedIDs.count, "merged ids unique")
    _ = try core.replace(parsed.library)
    check(try core.state().library == parsed.library, "replace adopts library")
    let protected = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/aText/x.json")
    do {
        _ = try core.export(path: protected.path)
        check(false, "export into aText data must be refused")
    } catch {
        check(error.localizedDescription.contains("Refusing"), "protected path error")
    }
}

test("system-wide expansion is off and is never started by these tests") {
    let core = try TypeluneCore(dataDirectory: tempDir())
    defer { core.close() }
    let status = try core.native(.status)
    check(status.state == .off, "off after open")
    check(status.stats.diag.keyEventsSeen == 0, "no events observed")
}

test("close is a lifetime barrier: concurrent calls never touch a freed core") {
    let core = try TypeluneCore(dataDirectory: tempDir())
    final class Counts: @unchecked Sendable {
        private let lock = NSLock()
        private(set) var succeeded = 0, closed = 0, other = 0
        func add(_ kind: Int) {
            lock.lock(); defer { lock.unlock() }
            switch kind {
            case 0: succeeded += 1;
            case 1: closed += 1;
            default: other += 1
            }
        }
    }
    let counts = Counts()
    let group = DispatchGroup()
    // Deterministic: every worker keeps calling until it observes "closed" (bounded), and
    // close() happens only after at least 200 calls succeeded, so close always races
    // in-flight calls regardless of machine speed.
    for _ in 0..<8 {
        DispatchQueue.global().async(group: group) {
            for i in 0..<1_000_000 {
                do {
                    _ = try core.render("n\(i) {{date}}")
                    counts.add(0)
                } catch let e as CoreError where e.message.contains("closed") {
                    counts.add(1)
                    return
                } catch {
                    counts.add(2)
                    return
                }
            }
        }
    }
    let deadline = Date().addingTimeInterval(10)
    while counts.succeeded < 200 && Date() < deadline { Thread.sleep(forTimeInterval: 0.001) }
    core.close()  // waits for in-flight calls, then frees
    core.close()  // idempotent
    group.wait()
    check(!core.isOpen, "closed")
    print("     lifetime: \(counts.succeeded) calls completed, then all 8 workers saw 'closed'")
    check(counts.succeeded >= 200, "calls were in flight when close() ran")
    check(counts.closed == 8, "every worker observed the closed core (none crashed or hung)")
    check(counts.other == 0, "no other errors")
    do {
        _ = try core.state()
        check(false, "calls after close must throw")
    } catch {
        check(error.localizedDescription.contains("closed"), "closed error after close")
    }
}

test("selection reconciles to a visible match or nothing") {
    check(SelectionRules.reconcile("a", visible: ["a", "b"]) == "a", "visible selection kept")
    check(SelectionRules.reconcile("a", visible: ["b", "c"]) == "b", "hidden selection moves to first match")
    check(SelectionRules.reconcile("a", visible: []) == nil, "no matches clears selection")
    check(SelectionRules.reconcile(nil, visible: ["b"]) == "b", "nothing selected picks first match")
}

print("\(passes) checks passed, \(failures) failed")
exit(failures == 0 ? 0 : 1)
