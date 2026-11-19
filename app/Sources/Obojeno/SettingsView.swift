import SwiftUI

/// Settings, in grouped sections like System Settings. Sliders preview while
/// dragging and save when released.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    var done: () -> Void

    @ViewState private var duration: Double?
    @ViewState private var grain: Double?
    @ViewState private var radius: Double?

    var body: some View {
        let state = model.state!
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Text("Settings").font(.title3.weight(.semibold))
                Spacer()
                Button("Done", action: done)
                    .buttonStyle(CapsuleButtonStyle())
                    .keyboardShortcut(.defaultAction)
            }

            SettingsGroup {
                ToggleRow(
                    "Open at login",
                    isOn: Binding(get: { model.openAtLogin }, set: { model.setOpenAtLogin($0) }))
                RowDivider()
                ToggleRow(
                    "Show in Dock",
                    isOn: Binding(get: { model.showInDock }, set: { model.showInDock = $0 }))
            }

            SectionTitle("Theme")
            SettingsGroup {
                Row("Scheme") {
                    Picker(
                        "Scheme",
                        selection: Binding(
                            get: { state.scheme }, set: { model.configure(ConfigChange(scheme: $0)) })
                    ) {
                        ForEach(SchemeVariant.allCases, id: \.self) { Text($0.label).tag($0) }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
                RowDivider()
                Row("Transition") {
                    Picker(
                        "Transition",
                        selection: Binding(
                            get: { state.transition },
                            set: { model.configure(ConfigChange(transition: $0)) })
                    ) {
                        ForEach(TransitionStyle.allCases, id: \.self) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .fixedSize()
                }
                RowDivider()
                SliderRow(
                    "Duration", value: duration ?? state.duration, in: 0.2...2.0,
                    format: { String(format: "%.1f s", $0) },
                    onChange: { duration = ($0 * 10).rounded() / 10 },
                    onCommit: {
                        if let duration { model.configure(ConfigChange(duration: duration)) }
                        duration = nil
                    }
                )
                .disabled(state.transition == .none)
            }
            if state.palette.variant != state.scheme {
                Caption("This wallpaper has no color, so it uses Monochrome.")
            }

            SectionTitle("Spaces")
            SettingsGroup {
                ToggleRow(
                    "Same wallpaper on every Space",
                    isOn: Binding(
                        get: { state.allSpaces }, set: { model.configure(ConfigChange(allSpaces: $0)) }))
            }
            Caption(
                "macOS only lets an app change the Space in view. obojeno brings the others up to date as you visit them."
            )

            SectionTitle("Finish")
            SettingsGroup {
                SliderRow(
                    "Grain", value: grain ?? state.finish.grain, in: 0...1,
                    format: { $0 < 0.005 ? "Off" : "\(Int(($0 * 100).rounded())) %" },
                    onChange: { grain = ($0 * 100).rounded() / 100 },
                    onCommit: {
                        if let grain { model.configure(ConfigChange(grain: grain)) }
                        grain = nil
                    })
                RowDivider()
                SliderRow(
                    "Rounded corners", value: radius ?? state.finish.cornerRadius, in: 0...30,
                    format: { $0 < 0.5 ? "Off" : "\(Int($0.rounded())) pt" },
                    onChange: { radius = $0.rounded() },
                    onCommit: {
                        if let radius { model.configure(ConfigChange(cornerRadius: radius)) }
                        radius = nil
                    })
            }
            Caption(
                "Grain softens banding in smooth wallpapers. Corners round the bottom of a MacBook's screen, and all four on other displays. Drag until they match the top of your screen."
            )

            SectionTitle("Lid")
            SettingsGroup {
                ToggleRow(
                    "Animate with the lid",
                    isOn: Binding(
                        get: { state.lidAnimation },
                        set: { model.configure(ConfigChange(lidAnimation: $0)) }))
            }
            Caption(
                "As you close your MacBook, darkness comes down over the wallpaper while it blurs and sinks back; opening brings it back."
            )

            SectionTitle("Folders")
            SettingsGroup {
                ForEach(state.wallpaperDirs, id: \.self) { dir in
                    HStack {
                        Image(systemName: "folder")
                        Text(dir).lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: 0)
                        Button {
                            model.removeFolder(dir)
                        } label: {
                            Image(systemName: "minus.circle.fill")
                        }
                        .buttonStyle(.plain)
                        .opacity(0.7)
                        .help("Stop showing \(dir)")
                    }
                    .frame(minHeight: 44)
                    RowDivider()
                }
                Button("Add Folder…") { model.addFolder() }
                    .buttonStyle(.plain)
                    .frame(minHeight: 44)
            }
            Caption("Imported images live in the library; folders here are shown where they are.")

            SectionTitle("Shortcuts")
            SettingsGroup {
                ShortcutRow("Next wallpaper", direction: .next)
                RowDivider()
                ShortcutRow("Previous wallpaper", direction: .previous)
                RowDivider()
                ShortcutRow("Random wallpaper", direction: .random)
            }
        }
    }
}

// MARK: - Building blocks

/// A rounded, translucent group of rows.
struct SettingsGroup<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) { content }
            .padding(.horizontal, 14)
            .glassEffect(.regular, in: .rect(cornerRadius: 16))
    }
}

struct SectionTitle: View {
    let title: String
    init(_ title: String) { self.title = title }

    var body: some View {
        Text(title)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.top, 4)
            .padding(.leading, 4)
    }
}

struct Caption: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 4)
    }
}

struct RowDivider: View {
    var body: some View { Divider().opacity(0.6) }
}

struct Row<Control: View>: View {
    let title: String
    @ViewBuilder let control: Control

    init(_ title: String, @ViewBuilder control: () -> Control) {
        self.title = title
        self.control = control()
    }

    var body: some View {
        HStack(spacing: 12) {
            Text(title)
            Spacer(minLength: 8)
            control
        }
        .frame(minHeight: 44)
    }
}

struct ToggleRow: View {
    let title: String
    let isOn: Binding<Bool>

    init(_ title: String, isOn: Binding<Bool>) {
        self.title = title
        self.isOn = isOn
    }

    var body: some View {
        Row(title) {
            Toggle(title, isOn: isOn).toggleStyle(.switch).labelsHidden()
        }
    }
}

struct SliderRow: View {
    let title: String
    let value: Double
    let range: ClosedRange<Double>
    let format: (Double) -> String
    let onChange: (Double) -> Void
    let onCommit: () -> Void

    init(
        _ title: String, value: Double, in range: ClosedRange<Double>, format: @escaping (Double) -> String,
        onChange: @escaping (Double) -> Void, onCommit: @escaping () -> Void
    ) {
        self.title = title
        self.value = value
        self.range = range
        self.format = format
        self.onChange = onChange
        self.onCommit = onCommit
    }

    var body: some View {
        Row(title) {
            Text(format(value))
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(width: 48, alignment: .trailing)
            // Continuous: a step would draw a tick mark per step.
            Slider(value: Binding(get: { value }, set: { onChange($0) }), in: range) { editing in
                if !editing { onCommit() }
            }
            .frame(width: 150)
        }
    }
}

/// Records a shortcut: click, then press the keys. Escape cancels.
struct ShortcutRow: View {
    @Environment(AppModel.self) private var model
    let title: String
    let direction: Direction
    @ViewState private var recording = false
    @ViewState private var monitor: Any?

    init(_ title: String, direction: Direction) {
        self.title = title
        self.direction = direction
    }

    var body: some View {
        let combo = model.shortcuts.combos[direction]
        Row(title) {
            HStack(spacing: 6) {
                Button {
                    recording ? stop() : start()
                } label: {
                    Text(recording ? "Press keys…" : combo?.label ?? "Record Shortcut")
                        .font(.callout.weight(combo == nil || recording ? .regular : .semibold))
                        .frame(minWidth: 96)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 5)
                        .background(
                            Capsule().fill(recording ? Color.accentColor.opacity(0.35) : .black.opacity(0.35)))
                }
                .buttonStyle(.plain)
                if combo != nil, !recording {
                    Button {
                        model.shortcuts.set(nil, for: direction)
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                    }
                    .buttonStyle(.plain)
                    .opacity(0.7)
                    .help("Remove the shortcut")
                }
            }
        }
        .onDisappear(perform: stop)
    }

    private func start() {
        recording = true
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            if event.keyCode == 53 {  // Escape
                stop()
            } else if let combo = KeyCombo(event: event) {
                model.shortcuts.set(combo, for: direction)
                stop()
            }
            return nil
        }
    }

    private func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
        recording = false
    }
}

struct CapsuleButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.callout.weight(.medium))
            .padding(.horizontal, 14)
            .padding(.vertical, 6)
            .glassEffect(.regular.interactive(), in: .capsule)
    }
}
