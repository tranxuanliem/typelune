// Pure Quick Search logic shared by the panel and the bridge tests: searching, ranking,
// keyboard selection and whether inserting (as opposed to copying) is allowed.

import Foundation

struct QuickSearchResult: Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let abbreviation: String
    let body: String
    let tags: [String]
    let groupName: String
    let active: Bool
}

enum QuickSearch {
    static let defaultLimit = 50

    /// Snippets matching `query` in title, abbreviation, body or tags. Best matches first:
    /// exact abbreviation, abbreviation prefix, title prefix, title, tag, body. Ties keep
    /// library order. An empty query lists everything in library order.
    static func results(in library: Library, query: String, limit: Int = defaultLimit) -> [QuickSearchResult] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        var scored: [(rank: Int, order: Int, result: QuickSearchResult)] = []
        var order = 0
        for group in library.groups {
            for s in group.snippets {
                order += 1
                guard let rank = rank(s, q) else { continue }
                let result = QuickSearchResult(
                    id: s.id, title: s.displayName, abbreviation: s.abbreviation, body: s.body, tags: s.tags,
                    groupName: group.name, active: s.enabled && group.enabled)
                scored.append((rank, order, result))
            }
        }
        scored.sort { ($0.rank, $0.order) < ($1.rank, $1.order) }
        return scored.prefix(max(0, limit)).map(\.result)
    }

    private static func rank(_ s: Snippet, _ q: String) -> Int? {
        if q.isEmpty { return 0 }
        let opts: String.CompareOptions = [.caseInsensitive, .diacriticInsensitive]
        if s.abbreviation.compare(q, options: opts) == .orderedSame { return 0 }
        if s.abbreviation.range(of: q, options: opts.union(.anchored)) != nil { return 1 }
        if s.displayName.range(of: q, options: opts.union(.anchored)) != nil { return 2 }
        if s.displayName.range(of: q, options: opts) != nil { return 3 }
        if s.abbreviation.range(of: q, options: opts) != nil { return 3 }
        if s.tags.contains(where: { $0.range(of: q, options: opts) != nil }) { return 4 }
        if s.body.range(of: q, options: opts) != nil { return 5 }
        return nil
    }

    /// Move the selection by `delta` rows, clamped to the list. With no selection, Down
    /// selects the first row and Up the last.
    static func move(_ selection: String?, by delta: Int, in ids: [String]) -> String? {
        guard !ids.isEmpty else { return nil }
        guard let selection, let i = ids.firstIndex(of: selection) else {
            return delta >= 0 ? ids.first : ids.last
        }
        return ids[min(max(i + delta, 0), ids.count - 1)]
    }
}

/// Whether Return types the snippet into the original field, or only copies it.
enum InsertAvailability: Equatable, Sendable {
    case insert
    case copyOnly(String)

    var canInsert: Bool { self == .insert }

    /// Mirrors the Rust checks (which are authoritative and run again before typing).
    /// `editsCommitted` is false when pending edits could not be handed to the core: the
    /// core could then type an older body than the one shown, so only Copy is offered.
    static func evaluate(
        native: NativeRunState?, target: CapturedTarget?, captureError: String?, editsCommitted: Bool = true
    ) -> InsertAvailability {
        if !editsCommitted {
            return .copyOnly(
                "Your latest edits could not be saved, so Insert is unavailable. Copy uses the text shown.")
        }
        guard let target else {
            return .copyOnly(captureError ?? "The field you came from is not known.")
        }
        if target.ownApp { return .copyOnly("Typelune itself was active.") }
        switch native ?? .off {
        case .running: break
        case .paused: return .copyOnly("System-wide expansion is paused. Resume it to insert.")
        case .stopping, .off, .error, .disabled:
            return .copyOnly("System-wide expansion is off. Start it to insert.")
        }
        if !target.hasFocusedElement {
            return .copyOnly("That app does not expose its text field, so Typelune cannot verify where text would go.")
        }
        if target.excluded { return .copyOnly("That app is excluded in Settings.") }
        if target.secureInput { return .copyOnly("Secure Keyboard Entry is active.") }
        return .insert
    }
}

/// How a Quick Search session ended.
enum QuickSearchDismissal: Equatable, Sendable {
    case escape
    case closeButton
    /// The global shortcut pressed again while the panel was open.
    case shortcut
    case copy
    case insert
    /// The user clicked another window or app.
    case clickAway
}

/// Focus rules for the Quick Search panel, which activates Typelune while it is open (the
/// only reliable way to own keyboard input) and hands focus back afterwards.
enum QuickSearchFocusPolicy {
    /// Give focus back to the app the user came from? Only after an explicit dismissal,
    /// only while Typelune is still the frontmost app (never over an app the user switched
    /// to), and only to a different, still-running app.
    static func restoreOrigin(
        after reason: QuickSearchDismissal, typeluneFrontmost: Bool, originRunning: Bool, originIsTypelune: Bool
    ) -> Bool {
        reason != .clickAway && typeluneFrontmost && originRunning && !originIsTypelune
    }

    /// Does the panel losing key status mean the user clicked away? Not while Typelune's
    /// own activation is still settling (AppKit may briefly re-key another Typelune window)
    /// and not when the panel is being hidden on purpose.
    static func resignKeyCancels(panelVisible: Bool, closingOnPurpose: Bool, activationPending: Bool) -> Bool {
        panelVisible && !closingOnPurpose && !activationPending
    }

    /// Another app became active while the panel was open: treat as click-away.
    static func otherAppActivationCancels(panelVisible: Bool, closingOnPurpose: Bool, activatedIsTypelune: Bool) -> Bool
    {
        panelVisible && !closingOnPurpose && !activatedIsTypelune
    }
}
