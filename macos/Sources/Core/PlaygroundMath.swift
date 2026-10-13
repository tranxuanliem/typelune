// Pure helpers that translate between text-view edits (UTF-16 ranges) and the Rust
// engine's model (Unicode scalars). Kept free of AppKit state so they can be tested.

import Foundation

enum PlaygroundMath {
    /// Largest edit treated as typing; larger edits (paste, drag) reset matching instead.
    static let maxTypedScalars = 4

    /// How a single text-view change should be fed to the engine.
    enum Feed: Equatable {
        case typed(backspaces: Int, inserted: String)
        case reset
    }

    /// Classify an edit. `removed`/`inserted` are the replaced and new text, `endsAtCaret`
    /// says whether the edit ends where the caret was, and `replacesMarkedText` marks an
    /// input-method commit (the marked text was never fed, so nothing is backspaced).
    static func classify(removed: String, inserted: String, endsAtCaret: Bool, replacesMarkedText: Bool) -> Feed {
        let removedCount = removed.unicodeScalars.count
        let insertedCount = inserted.unicodeScalars.count
        if replacesMarkedText {
            return insertedCount <= maxTypedScalars * 4 ? .typed(backspaces: 0, inserted: inserted) : .reset
        }
        guard endsAtCaret, removedCount <= maxTypedScalars, insertedCount <= maxTypedScalars else {
            return .reset
        }
        if removedCount == 0 && insertedCount == 0 { return .reset }
        return .typed(backspaces: removedCount, inserted: inserted)
    }

    /// UTF-16 length of the first `scalars` Unicode scalars of `text`.
    static func utf16Prefix(of text: String, scalars: Int) -> Int {
        var units = 0
        for (i, scalar) in text.unicodeScalars.enumerated() {
            if i >= scalars { break }
            units += String(scalar).utf16.count
        }
        return units
    }

    struct Replacement: Equatable {
        var range: NSRange
        var text: String
        var caret: Int
    }

    /// Where to apply an engine expansion in `text` (an NSString-compatible UTF-16 view).
    /// `editStart` is the UTF-16 location of the edit that triggered it; `inserted` is the
    /// inserted text. Returns nil if the text before the caret is not exactly what Rust
    /// says was typed (never delete something else).
    static func replacement(in text: String, editStart: Int, inserted: String, expansion: PlaygroundExpansion)
        -> Replacement?
    {
        let ns = text as NSString
        let caret = editStart + utf16Prefix(of: inserted, scalars: expansion.consumedChars)
        let deleteLength = (expansion.deleteText as NSString).length
        let start = caret - deleteLength
        guard start >= 0, caret <= ns.length else { return nil }
        let range = NSRange(location: start, length: deleteLength)
        guard ns.substring(with: range) == expansion.deleteText else { return nil }
        let newCaret = start + (expansion.insert as NSString).length - (expansion.afterCursor as NSString).length
        return Replacement(range: range, text: expansion.insert, caret: newCaret)
    }
}
