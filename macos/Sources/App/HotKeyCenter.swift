// One global keyboard shortcut through Carbon's RegisterEventHotKey. It delivers only the
// registered key combination (no other typing), needs no permission, and works while
// Typelune is in the background.

import AppKit
import Carbon

@MainActor
final class HotKeyCenter {
    enum Status: Equatable {
        case none
        case registered(String)
        case failed(String)
    }

    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private var action: (() -> Void)?
    private(set) var status: Status = .none

    /// Replace the registered shortcut. `nil` removes it.
    @discardableResult
    func register(_ hotkey: Hotkey?, action: @escaping () -> Void) -> Status {
        unregister()
        self.action = action
        guard let hotkey else {
            status = .none
            return status
        }
        if let problem = HotkeyRules.problem(hotkey) {
            status = .failed(problem)
            return status
        }
        if let error = installHandlerIfNeeded() {
            status = .failed(error)
            return status
        }
        var modifiers: UInt32 = 0
        if hotkey.command { modifiers |= UInt32(cmdKey) }
        if hotkey.option { modifiers |= UInt32(optionKey) }
        if hotkey.control { modifiers |= UInt32(controlKey) }
        if hotkey.shift { modifiers |= UInt32(shiftKey) }
        let id = EventHotKeyID(signature: OSType(0x5450_4C51), id: 1)  // 'TPLQ'
        var newRef: EventHotKeyRef?
        let err = RegisterEventHotKey(
            UInt32(hotkey.keyCode), modifiers, id, GetApplicationEventTarget(), OptionBits(kEventHotKeyExclusive),
            &newRef)
        let shown = HotkeyRules.display(hotkey)
        switch err {
        case noErr:
            ref = newRef
            status = .registered(shown)
        case OSStatus(eventHotKeyExistsErr):
            status = .failed("\(shown) is already taken by another app. Choose a different shortcut.")
        default:
            status = .failed("\(shown) could not be registered (error \(err)).")
        }
        return status
    }

    func unregister() {
        if let ref { UnregisterEventHotKey(ref) }
        ref = nil
        status = .none
    }

    fileprivate func fire() { action?() }

    private func installHandlerIfNeeded() -> String? {
        if handler != nil { return nil }
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let me = Unmanaged.passUnretained(self).toOpaque()
        let err = InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, userData in
                guard let userData else { return OSStatus(eventNotHandledErr) }
                let center = Unmanaged<HotKeyCenter>.fromOpaque(userData).takeUnretainedValue()
                // Application-target Carbon events are dispatched on the main thread.
                MainActor.assumeIsolated { center.fire() }
                return noErr
            }, 1, &spec, me, &handler)
        return err == noErr ? nil : "The shortcut handler could not be installed (error \(err))."
    }
}
