import SwiftUI

struct PopoverView: View {
    @Environment(AppModel.self) private var model
    @ViewState private var showingSettings: Bool

    init(startWithSettings: Bool = false) {
        _showingSettings = ViewState(initialValue: startWithSettings)
    }

    var body: some View {
        VStack(spacing: 0) {
            if model.connected, model.state != nil {
                content
            } else {
                DisconnectedView()
            }
        }
        .frame(width: 380)
        .tint(model.color("primary"))
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 12) {
            HeaderView()
            Controls()
            Divider()
            if showingSettings {
                SettingsView()
            } else {
                GalleryView()
            }
            Divider()
            Footer(showingSettings: $showingSettings)
        }
        .padding(14)
    }
}

private struct HeaderView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let state = model.state!
        HStack(spacing: 12) {
            Thumbnail(path: model.currentItem?.thumbnail, fallback: model.color("surface_container_high"))
                .frame(width: 104, height: 65)
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(.primary.opacity(0.12)))
            VStack(alignment: .leading, spacing: 6) {
                Text(title(state))
                    .font(.headline)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text("\(state.palette.variant.label) · \(state.palette.source.uppercased())")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
                HStack(spacing: 4) {
                    ForEach(["primary", "secondary", "tertiary", "primary_container", "surface_container_high"], id: \.self) {
                        role in
                        Circle()
                            .fill(model.color(role))
                            .overlay(Circle().strokeBorder(.primary.opacity(0.15)))
                            .frame(width: 16, height: 16)
                            .help(role.replacingOccurrences(of: "_", with: " "))
                    }
                }
            }
            Spacer(minLength: 0)
        }
    }

    private func title(_ state: DaemonState) -> String {
        if let color = state.fixedColor { return "Fixed color \(color.uppercased())" }
        guard let wallpaper = state.wallpaper else { return "No wallpaper" }
        return URL(fileURLWithPath: wallpaper).deletingPathExtension().lastPathComponent
    }
}

private struct Controls: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 10) {
            Picker(
                "Appearance",
                selection: Binding(get: { model.mode }, set: { model.setMode($0) })
            ) {
                Label("Light", systemImage: "sun.max").tag(Mode.light)
                Label("Dark", systemImage: "moon").tag(Mode.dark)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()

            Spacer(minLength: 0)

            Button {
                model.chooseImage()
            } label: {
                Label("Choose Image…", systemImage: "photo.badge.plus")
            }
        }
    }
}

private struct Footer: View {
    @Environment(AppModel.self) private var model
    @Binding var showingSettings: Bool

    var body: some View {
        HStack(spacing: 8) {
            Button {
                withAnimation(.snappy) { showingSettings.toggle() }
            } label: {
                Label(showingSettings ? "Gallery" : "Settings",
                      systemImage: showingSettings ? "photo.on.rectangle" : "gearshape")
            }
            .buttonStyle(.borderless)

            if let error = model.error {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .lineLimit(2)
                    .help(error)
            } else if let pending = model.gallery?.pending, pending > 0 {
                ProgressView().controlSize(.small)
                Text("Indexing \(pending)…").font(.caption).foregroundStyle(.secondary)
            }

            Spacer(minLength: 0)

            Button("Quit") { NSApp.terminate(nil) }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
        }
    }
}

private struct DisconnectedView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 10) {
            Image(systemName: "paintpalette")
                .font(.system(size: 28))
                .foregroundStyle(.secondary)
            Text("obojeno isn't running")
                .font(.headline)
            Text("The daemon themes your apps and serves this menu.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Button("Start obojeno") { model.startDaemon() }
                .keyboardShortcut(.defaultAction)
            if let error = model.error {
                Text(error).font(.caption).foregroundStyle(.red).multilineTextAlignment(.center)
            }
            Divider().padding(.vertical, 4)
            Button("Quit") { NSApp.terminate(nil) }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
        }
        .padding(20)
        .frame(maxWidth: .infinity)
    }
}
