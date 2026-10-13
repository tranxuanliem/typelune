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

test("preferences are isolated in the data folder, safe by default, and round-trip") {
    let dir = tempDir()
    let core = try TypeluneCore(dataDirectory: dir)
    defer { core.close() }
    let reply = try core.preferences()
    check(reply.isolated, "an explicit data folder is isolated (no login items)")
    check(!reply.preferences.startExpansionAtLaunch, "auto-start is off by default")
    check(reply.preferences.excludedApps.isEmpty, "no exclusions by default")
    check(
        !FileManager.default.fileExists(atPath: dir.appendingPathComponent("preferences.json").path),
        "reading preferences writes nothing")
    var prefs = reply.preferences
    prefs.quickSearchHotkey = nil
    prefs.excludedApps = [ExcludedApp(bundleId: "com.example.Editor", name: "Editor")]
    _ = try core.setPreferences(prefs)
    let reopened = try TypeluneCore(dataDirectory: dir)
    defer { reopened.close() }
    let again = try reopened.preferences().preferences
    check(again.quickSearchHotkey == nil, "a cleared shortcut stays cleared (encoded as null)")
    check(again.excludedApps.map(\.bundleId) == ["com.example.Editor"], "exclusions persisted")
}

test("v0.3 libraries open unchanged; preferences never touch library.json") {
    let dir = tempDir()
    try TypeluneCore(dataDirectory: dir).close()  // creates a v1 library like v0.3 did
    let file = dir.appendingPathComponent("library.json")
    let before = try Data(contentsOf: file)
    let core = try TypeluneCore(dataDirectory: dir)
    var prefs = try core.preferences().preferences
    prefs.startExpansionAtLaunch = true
    _ = try core.setPreferences(prefs)
    _ = try core.backups()
    core.close()
    check(try Data(contentsOf: file) == before, "library bytes unchanged")
    check(
        String(decoding: before, as: UTF8.self).contains("\"atext-rs.library\""),
        "schema literal unchanged (v1)")
}

test("backup now, preview and restore keep the current library first") {
    let dir = tempDir()
    let core = try TypeluneCore(dataDirectory: dir)
    defer { core.close() }
    let original = try core.state().library
    let made = try core.backupNow()
    guard let name = made.created else {
        check(false, "backup created")
        return
    }
    check(made.backups.count == 1 && made.retention == 30, "listed with retention 30")
    var edited = original
    edited.groups[0].name = "Edited"
    _ = try core.update(edited)
    check(try core.readBackup(name).library == original, "preview shows the snapshot")
    let restored = try core.restoreBackup(name)
    check(restored.library == original, "restored")
    check(try core.readBackup(restored.preRestore).library.groups[0].name == "Edited", "pre-restore kept")
    do {
        _ = try core.restoreBackup("../library.json")
        check(false, "path names are refused")
    } catch {
        check(try core.state().library == original, "failed restore changes nothing")
    }
}

test("CSV preview reports rows and never changes the library") {
    let dir = tempDir()
    let core = try TypeluneCore(dataDirectory: dir)
    defer { core.close() }
    let before = try core.state().library
    let csv = dir.appendingPathComponent("in.csv")
    try Data("\u{FEFF}abbreviation,body,group\r\nzz1,\"a, \"\"b\"\"\",G\r\n,empty,G\r\nzz1,dup,G\r\n".utf8).write(
        to: csv)
    let parsed = try core.parseCsvImport(path: csv.path)
    check(parsed.report.imported == 1 && parsed.report.skippedCount == 2, "one imported, two skipped")
    check(parsed.library.groups.first?.snippets.first?.body == "a, \"b\"", "quoted comma and quotes")
    check(parsed.report.skipped.map(\.line) == [3, 4], "skipped lines reported")
    check(try core.state().library == before, "library unchanged until confirmed")
    let empty = dir.appendingPathComponent("empty.csv")
    try Data("abbreviation,body\n".utf8).write(to: empty)
    check(try core.parseCsvImport(path: empty.path).report.imported == 0, "header-only file imports nothing")
}

test("quick search ranks matches and moves the selection") {
    let core = try TypeluneCore(dataDirectory: tempDir())
    defer { core.close() }
    let lib = try core.state().library
    let byAbbr = QuickSearch.results(in: lib, query: "sig")
    check(byAbbr.first?.abbreviation == "sig", "exact abbreviation first")
    check(QuickSearch.results(in: lib, query: "SIGN-OFF").first?.title == "Sign-off", "title, case-insensitive")
    check(QuickSearch.results(in: lib, query: "greeting").contains { $0.abbreviation == "hlo" }, "tags searched")
    check(QuickSearch.results(in: lib, query: "Alex Example").contains { $0.abbreviation == "sig" }, "body searched")
    check(QuickSearch.results(in: lib, query: "zzzz-none").isEmpty, "no match")
    check(QuickSearch.results(in: lib, query: "", limit: 3).count == 3, "limit")
    let archived = QuickSearch.results(in: lib, query: "hlo2")
    check(archived.first?.active == false, "disabled snippets are listed and marked")
    let ids = ["a", "b", "c"]
    check(QuickSearch.move(nil, by: 1, in: ids) == "a", "down from nothing selects first")
    check(QuickSearch.move(nil, by: -1, in: ids) == "c", "up from nothing selects last")
    check(QuickSearch.move("c", by: 1, in: ids) == "c", "clamped at end")
    check(QuickSearch.move("a", by: -1, in: ids) == "a", "clamped at start")
    check(QuickSearch.move("b", by: 1, in: []) == nil, "empty list")
}

test("insert is offered only for a running, verified, allowed target; copy always works") {
    let ok = CapturedTarget(
        token: 1, pid: 42, bundleId: "com.example.Editor", excluded: false, secureInput: false, ownApp: false,
        hasFocusedElement: true)
    check(InsertAvailability.evaluate(native: .running, target: ok, captureError: nil) == .insert, "allowed")
    func copyOnly(_ state: NativeRunState?, _ t: CapturedTarget?) -> Bool {
        !InsertAvailability.evaluate(native: state, target: t, captureError: "x").canInsert
    }
    check(copyOnly(.off, ok), "off")
    check(copyOnly(.paused, ok), "paused")
    check(copyOnly(.stopping, ok), "stopping")
    check(copyOnly(nil, ok), "unknown state")
    check(copyOnly(.running, nil), "no target (e.g. no Accessibility)")
    var t = ok
    t.hasFocusedElement = false
    check(copyOnly(.running, t), "field not identified exactly")
    t = ok
    t.excluded = true
    check(copyOnly(.running, t), "excluded app")
    t = ok
    t.secureInput = true
    check(copyOnly(.running, t), "secure input")
    t = ok
    t.ownApp = true
    check(copyOnly(.running, t), "Typelune itself")
    check(
        !InsertAvailability.evaluate(native: .running, target: ok, captureError: nil, editsCommitted: false).canInsert,
        "pending edits that could not be committed block Insert")
}

test("Rust refuses quick insertion when off and keeps the run state while inhibited") {
    let core = try TypeluneCore(dataDirectory: tempDir())
    defer { core.close() }
    let id = try core.state().library.groups[0].snippets[0].id
    do {
        _ = try core.quickInsert(token: 1, snippetID: id)
        check(false, "must be refused while expansion is off")
    } catch {
        check(error.localizedDescription.contains("off"), "honest reason")
    }
    let on = try core.setInhibited(true)
    check(on.inhibited && on.state == .off, "inhibited without starting anything")
    check(try !core.setInhibited(false).inhibited, "released")
    check(try core.native(.status).stats.diag.quickInsertsQueued == 0, "nothing queued")
}

test("quick search focus: restore only after explicit dismissal, never over another app") {
    func restore(_ r: QuickSearchDismissal, front: Bool = true, running: Bool = true, own: Bool = false) -> Bool {
        QuickSearchFocusPolicy.restoreOrigin(
            after: r, typeluneFrontmost: front, originRunning: running, originIsTypelune: own)
    }
    for r in [QuickSearchDismissal.escape, .closeButton, .shortcut, .copy, .insert] {
        check(restore(r), "explicit dismissal returns focus: \(r)")
        check(!restore(r, front: false), "never over an app the user switched to: \(r)")
        check(!restore(r, running: false), "origin quit: \(r)")
        check(!restore(r, own: true), "origin was Typelune itself: \(r)")
    }
    check(!restore(.clickAway), "clicking elsewhere keeps the user's choice")
    check(
        !QuickSearchFocusPolicy.resignKeyCancels(panelVisible: true, closingOnPurpose: false, activationPending: true),
        "activation still settling: losing key status is not a cancel")
    check(
        QuickSearchFocusPolicy.resignKeyCancels(panelVisible: true, closingOnPurpose: false, activationPending: false),
        "after activation, losing key status is a click-away")
    check(
        !QuickSearchFocusPolicy.resignKeyCancels(panelVisible: true, closingOnPurpose: true, activationPending: false),
        "hiding on purpose is not a click-away")
    check(
        !QuickSearchFocusPolicy.resignKeyCancels(
            panelVisible: false, closingOnPurpose: false, activationPending: false),
        "hidden panel")
    check(
        QuickSearchFocusPolicy.otherAppActivationCancels(
            panelVisible: true, closingOnPurpose: false, activatedIsTypelune: false),
        "another app activated while open: click-away")
    check(
        !QuickSearchFocusPolicy.otherAppActivationCancels(
            panelVisible: true, closingOnPurpose: false, activatedIsTypelune: true),
        "Typelune's own activation is expected")
    check(
        !QuickSearchFocusPolicy.otherAppActivationCancels(
            panelVisible: false, closingOnPurpose: true, activatedIsTypelune: false),
        "returning focus to the origin after Copy/Insert is not a click-away")
}

test("shortcut, auto-start and login-item rules") {
    let def = Hotkey(keyCode: 17, command: true, option: true, control: true, shift: false, keyLabel: "T")
    check(HotkeyRules.problem(def) == nil, "default is valid")
    check(HotkeyRules.display(def) == "⌃⌥⌘T", "display order")
    var h = def
    h.command = false
    h.option = false
    h.control = false
    check(HotkeyRules.problem(h) != nil, "needs a modifier")
    h = Hotkey(keyCode: 17, command: true, option: false, control: false, shift: false, keyLabel: "T")
    check(HotkeyRules.problem(h) != nil, "plain ⌘-key is refused")
    h = Hotkey(keyCode: 49, command: false, option: true, control: false, shift: false, keyLabel: "Space")
    check(HotkeyRules.problem(h) != nil, "⌥Space alone is refused")
    check(!LaunchPolicy.shouldAutoStart(optedIn: false, accessibilityGranted: true), "no opt-in, no start")
    check(!LaunchPolicy.shouldAutoStart(optedIn: true, accessibilityGranted: false), "never without permission")
    check(LaunchPolicy.shouldAutoStart(optedIn: true, accessibilityGranted: true), "opt-in and permission")
    check(!LaunchPolicy.loginItemsAllowed(isolated: true), "isolated runs never register login items")
    check(LaunchPolicy.loginItemsAllowed(isolated: false), "default folder may")
}

print("\(passes) checks passed, \(failures) failed")
exit(failures == 0 ? 0 : 1)
