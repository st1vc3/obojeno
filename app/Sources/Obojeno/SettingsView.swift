import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    /// Follows the slider while dragging; saved when the drag ends.
    @ViewState private var duration: Double?

    var body: some View {
        let state = model.state!
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 14) {
                GridRow {
                    Text("Scheme").gridColumnAlignment(.trailing)
                    Picker(
                        "Scheme",
                        selection: Binding(
                            get: { state.scheme },
                            set: { model.configure(ConfigChange(scheme: $0)) })
                    ) {
                        ForEach(SchemeVariant.allCases, id: \.self) { Text($0.label).tag($0) }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
                if state.palette.variant != state.scheme {
                    GridRow {
                        Color.clear.gridCellUnsizedAxes([.horizontal, .vertical])
                        Text("This wallpaper has no color, so it uses Monochrome.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                GridRow {
                    Text("Transition")
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
                }
                GridRow {
                    Text("Duration")
                    HStack {
                        // Continuous: a step would draw a tick mark per 0.1 s.
                        Slider(
                            value: Binding(
                                get: { duration ?? state.duration },
                                set: { duration = ($0 * 10).rounded() / 10 }),
                            in: 0.2...2.0
                        ) { editing in
                            if !editing, let duration {
                                model.configure(ConfigChange(duration: duration))
                                self.duration = nil
                            }
                        }
                        Text(String(format: "%.1f s", duration ?? state.duration))
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                            .frame(width: 40, alignment: .trailing)
                    }
                    .disabled(state.transition == .none)
                }
                GridRow(alignment: .top) {
                    Text("Folders")
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(state.wallpaperDirs, id: \.self) { dir in
                            HStack {
                                Image(systemName: "folder").foregroundStyle(.secondary)
                                Text(dir).lineLimit(1).truncationMode(.middle)
                                Spacer(minLength: 0)
                                Button {
                                    model.removeFolder(dir)
                                } label: {
                                    Image(systemName: "minus.circle.fill")
                                }
                                .buttonStyle(.borderless)
                                .foregroundStyle(.secondary)
                                .help("Remove \(dir)")
                            }
                        }
                        Button("Add Folder…") { model.addFolder() }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 4)
    }
}
