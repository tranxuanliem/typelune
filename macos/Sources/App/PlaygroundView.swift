import AppKit
import SwiftUI

struct PlaygroundView: View {
    @Environment(AppModel.self) private var model
    @State private var clearToken = 0
    @State private var showBuffer = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(
                "Type below. Abbreviations from your current snippets expand here. Nothing typed here leaves this window, and system-wide expansion ignores it."
            )
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            PlaygroundTextView(clearToken: clearToken)
                .frame(minHeight: 160)
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.separator))
                .accessibilityLabel("Playground text")
            HStack {
                Button("Clear", systemImage: "trash") {
                    clearToken += 1
                    model.playgroundReset()
                    model.clearPlaygroundLog()
                }
                Toggle("Show matching buffer", isOn: $showBuffer)
                Spacer()
                Text("Default trigger: \(model.library.settings.defaultTrigger.title)")
                    .foregroundStyle(.secondary)
            }
            if showBuffer {
                Text("Buffer: \(model.playgroundBuffer.debugDescription)")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            }
            GroupBox("Recent Expansions") {
                if model.playgroundLog.isEmpty {
                    Text("None yet. Try typing “hlo” and a space.")
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(model.playgroundLog.prefix(8)) { entry in
                            HStack {
                                Text(entry.time, format: .dateTime.hour().minute().second())
                                    .monospacedDigit()
                                    .foregroundStyle(.secondary)
                                Text(entry.title)
                                if !entry.warnings.isEmpty {
                                    Image(systemName: "exclamationmark.triangle.fill")
                                        .symbolRenderingMode(.multicolor)
                                        .help(entry.warnings.joined(separator: "\n"))
                                        .accessibilityLabel("Warning")
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .padding()
        .navigationTitle("Playground")
        .onAppear { model.playgroundReset() }
    }
}

/// NSTextView bridge. Needed because the Playground must see each individual edit and
/// place the caret after an expansion, which SwiftUI's TextEditor does not expose on
/// macOS 15 without diffing whole strings.
private struct PlaygroundTextView: NSViewRepresentable {
    @Environment(AppModel.self) private var model
    let clearToken: Int

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        let textView = scroll.documentView as! NSTextView
        textView.delegate = context.coordinator
        textView.isRichText = false
        textView.allowsUndo = true
        textView.font = .monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.textContainerInset = NSSize(width: 6, height: 6)
        context.coordinator.textView = textView
        return scroll
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {
        if context.coordinator.clearToken != clearToken {
            context.coordinator.clearToken = clearToken
            context.coordinator.clear()
        }
    }

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        let model: AppModel
        weak var textView: NSTextView?
        var clearToken = 0
        private var pending: (feed: PlaygroundMath.Feed, start: Int)?
        private var applying = false
        private var expectedSelection = NSRange(location: 0, length: 0)

        init(model: AppModel) { self.model = model }

        func clear() {
            guard let textView else { return }
            applying = true
            textView.string = ""
            applying = false
            expectedSelection = textView.selectedRange()
        }

        func textView(_ textView: NSTextView, shouldChangeTextIn range: NSRange, replacementString: String?) -> Bool {
            guard !applying, let replacement = replacementString else { return true }
            let removed = (textView.string as NSString).substring(with: range)
            let selection = textView.selectedRange()
            let feed = PlaygroundMath.classify(
                removed: removed,
                inserted: replacement,
                endsAtCaret: NSMaxRange(range) == NSMaxRange(selection),
                replacesMarkedText: textView.hasMarkedText())
            pending = (feed, range.location)
            return true
        }

        func textDidChange(_ notification: Notification) {
            guard !applying, let textView else { return }
            defer { expectedSelection = textView.selectedRange() }
            if textView.hasMarkedText() {
                pending = nil  // input method is composing; feed on commit
                return
            }
            guard let (feed, start) = pending else { return }
            pending = nil
            switch feed {
            case .reset:
                model.playgroundReset()
            case .typed(let backspaces, let inserted):
                guard let exp = model.playgroundEdit(backspaces: backspaces, inserted: inserted) else { return }
                apply(exp, editStart: start, inserted: inserted, in: textView)
            }
        }

        func textViewDidChangeSelection(_ notification: Notification) {
            guard !applying, pending == nil, let textView else { return }
            let selection = textView.selectedRange()
            if selection != expectedSelection {
                model.playgroundReset()  // click or arrow key: typing context is unknown
                expectedSelection = selection
            }
        }

        private func apply(_ exp: PlaygroundExpansion, editStart: Int, inserted: String, in textView: NSTextView) {
            guard
                let rep = PlaygroundMath.replacement(
                    in: textView.string, editStart: editStart, inserted: inserted, expansion: exp)
            else {
                model.playgroundReset()
                return
            }
            applying = true
            if textView.shouldChangeText(in: rep.range, replacementString: rep.text) {
                textView.replaceCharacters(in: rep.range, with: rep.text)
                textView.didChangeText()
            }
            textView.setSelectedRange(NSRange(location: rep.caret, length: 0))
            applying = false
        }
    }
}
