// Pure selection rules shared by the app and the bridge tests.

enum SelectionRules {
    /// Keep `current` if it is still visible; otherwise select the first visible row, or
    /// nothing when no row is visible (so commands never target a hidden snippet).
    static func reconcile(_ current: String?, visible: [String]) -> String? {
        if let current, visible.contains(current) { return current }
        return visible.first
    }
}
