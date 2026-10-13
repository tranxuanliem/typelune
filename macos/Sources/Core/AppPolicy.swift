// Pure rules for global shortcuts, login items and starting expansion at launch.

import Foundation

enum HotkeyRules {
    /// Virtual key codes that never make a usable global shortcut on their own.
    private static let reserved: Set<Int> = [
        36, 48, 49, 51, 53, 76, 117, 123, 124, 125, 126,  // Return, Tab, Space, Delete, Esc, Enter, Fwd-Del, arrows
    ]

    /// `nil` when valid, otherwise why the shortcut is refused.
    static func problem(_ h: Hotkey) -> String? {
        if !(h.command || h.option || h.control) {
            return "Use at least one of Command, Option or Control."
        }
        if h.keyCode < 0 || h.keyCode > 0x7F { return "That key cannot be used." }
        if reserved.contains(h.keyCode) && !(h.command && h.option || h.control && h.option) {
            return "That key needs at least two modifiers (for example ⌥⌘)."
        }
        if h.command && !h.option && !h.control && !h.shift {
            return "⌘ with a single key is used by apps; add ⌥ or ⌃."
        }
        return nil
    }

    static func display(_ h: Hotkey) -> String {
        var s = ""
        if h.control { s += "⌃" }
        if h.option { s += "⌥" }
        if h.shift { s += "⇧" }
        if h.command { s += "⌘" }
        return s + (h.keyLabel.isEmpty ? "Key \(h.keyCode)" : h.keyLabel.uppercased())
    }
}

enum LaunchPolicy {
    /// Expansion starts at launch only after an explicit, remembered opt-in, and only if
    /// Accessibility is already granted (Typelune never asks for or grants it).
    static func shouldAutoStart(optedIn: Bool, accessibilityGranted: Bool) -> Bool {
        optedIn && accessibilityGranted
    }

    /// A run with an explicit data folder (`TYPELUNE_DATA_DIR`, tests, QA) must never
    /// register the real app as a login item.
    static func loginItemsAllowed(isolated: Bool) -> Bool { !isolated }
}
