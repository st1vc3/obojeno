// Global keyboard shortcuts for next, previous and random wallpaper.
// Carbon's RegisterEventHotKey needs no Accessibility permission, unlike
// event taps or global key monitors.

import AppKit
import Carbon.HIToolbox
import Observation

/// A key with modifiers, stored as Carbon key code and modifier flags.
struct KeyCombo: Codable, Equatable {
    let keyCode: UInt32
    let modifiers: UInt32

    init(keyCode: UInt32, modifiers: UInt32) {
        self.keyCode = keyCode
        self.modifiers = modifiers
    }

    /// From a key press; nil without a command, option or control
    /// modifier, since plain keys would block typing everywhere.
    init?(event: NSEvent) {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard !flags.intersection([.command, .option, .control]).isEmpty else { return nil }
        var carbon: UInt32 = 0
        if flags.contains(.command) { carbon |= UInt32(cmdKey) }
        if flags.contains(.option) { carbon |= UInt32(optionKey) }
        if flags.contains(.control) { carbon |= UInt32(controlKey) }
        if flags.contains(.shift) { carbon |= UInt32(shiftKey) }
        self.init(keyCode: UInt32(event.keyCode), modifiers: carbon)
    }

    /// Like the menus show it: ⌃⌥⇧⌘ then the key.
    var label: String {
        var text = ""
        if modifiers & UInt32(controlKey) != 0 { text += "⌃" }
        if modifiers & UInt32(optionKey) != 0 { text += "⌥" }
        if modifiers & UInt32(shiftKey) != 0 { text += "⇧" }
        if modifiers & UInt32(cmdKey) != 0 { text += "⌘" }
        return text + Self.keyName(keyCode)
    }

    private static func keyName(_ code: UInt32) -> String {
        let special: [Int: String] = [
            kVK_Return: "↩", kVK_Tab: "⇥", kVK_Space: "Space", kVK_Delete: "⌫", kVK_Escape: "⎋",
            kVK_LeftArrow: "←", kVK_RightArrow: "→", kVK_UpArrow: "↑", kVK_DownArrow: "↓",
        ]
        if let name = special[Int(code)] { return name }
        // The character this key types on the current keyboard layout.
        guard let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
            let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData)
        else { return "?" }
        let layout = unsafeBitCast(data, to: CFData.self)
        var dead: UInt32 = 0
        var chars = [UniChar](repeating: 0, count: 4)
        var length = 0
        let status = CFDataGetBytePtr(layout).withMemoryRebound(to: UCKeyboardLayout.self, capacity: 1) {
            UCKeyTranslate(
                $0, UInt16(code), UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()),
                OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, chars.count, &length, &chars)
        }
        return status == noErr && length > 0 ? String(utf16CodeUnits: chars, count: length).uppercased() : "?"
    }
}

@MainActor @Observable
final class Shortcuts {
    /// Called with the action of a pressed shortcut.
    @ObservationIgnored var onTrigger: ((Direction) -> Void)?

    private(set) var combos: [Direction: KeyCombo] = [:]
    @ObservationIgnored private var registered: [Direction: EventHotKeyRef] = [:]
    @ObservationIgnored private var handler: EventHandlerRef?

    static let all: [Direction] = [.next, .previous, .random]

    init() {
        for direction in Self.all {
            if let data = UserDefaults.standard.data(forKey: Self.key(direction)),
                let combo = try? JSONDecoder().decode(KeyCombo.self, from: data)
            {
                combos[direction] = combo
            }
        }
    }

    private static func key(_ direction: Direction) -> String { "shortcut.\(direction.rawValue)" }

    func set(_ combo: KeyCombo?, for direction: Direction) {
        combos[direction] = combo
        if let combo, let data = try? JSONEncoder().encode(combo) {
            UserDefaults.standard.set(data, forKey: Self.key(direction))
        } else {
            UserDefaults.standard.removeObject(forKey: Self.key(direction))
        }
        registerAll()
    }

    /// (Re)registers every shortcut with the system.
    func registerAll() {
        installHandler()
        for (_, ref) in registered { UnregisterEventHotKey(ref) }
        registered.removeAll()
        for (index, direction) in Self.all.enumerated() {
            guard let combo = combos[direction] else { continue }
            let id = EventHotKeyID(signature: OSType(0x6F_62_6F_6A), id: UInt32(index))  // 'oboj'
            var ref: EventHotKeyRef?
            let status = RegisterEventHotKey(
                combo.keyCode, combo.modifiers, id, GetApplicationEventTarget(), 0, &ref)
            if status == noErr, let ref { registered[direction] = ref }
        }
    }

    private func installHandler() {
        guard handler == nil else { return }
        var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let me = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(
            GetApplicationEventTarget(),
            { _, event, context in
                var id = EventHotKeyID()
                GetEventParameter(
                    event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                    MemoryLayout<EventHotKeyID>.size, nil, &id)
                guard let context, Int(id.id) < Shortcuts.all.count else { return noErr }
                let shortcuts = Unmanaged<Shortcuts>.fromOpaque(context).takeUnretainedValue()
                let direction = Shortcuts.all[Int(id.id)]
                MainActor.assumeIsolated { shortcuts.onTrigger?(direction) }
                return noErr
            }, 1, &type, me, &handler)
    }
}
