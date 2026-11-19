import SwiftUI

/// The popover's size: large enough to see a good part of the gallery at
/// once without scrolling.
enum PopoverSize {
    static let width: CGFloat = 460
    /// The scrolling area under the header (gallery or settings).
    static let contentHeight: CGFloat = 640
}

struct PopoverView: View {
    @Environment(AppModel.self) private var model
    @ViewState private var showingSettings: Bool

    init(startWithSettings: Bool = false) {
        _showingSettings = ViewState(initialValue: startWithSettings)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if model.connected, model.state != nil {
                if showingSettings {
                    ScrollView {
                        SettingsView { withAnimation(.snappy) { showingSettings = false } }
                            .padding(.bottom, 4)
                    }
                    .scrollIndicators(.never)
                    .frame(height: PopoverSize.contentHeight + 44)
                } else {
                    Header(showSettings: { withAnimation(.snappy) { showingSettings = true } })
                    GalleryView()
                }
                if let error = model.error {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(.white)
                        .lineLimit(2)
                        .help(error)
                }
            } else {
                DisconnectedView()
            }
        }
        .padding(14)
        .frame(width: PopoverSize.width)
        .background { Tint() }
        .environment(\.colorScheme, .dark)
        .tint(.white)
    }
}

/// The wallpaper's own color over the popover's glass, so the menu looks
/// cut from the wallpaper.
private struct Tint: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let palette = model.state?.palette {
            LinearGradient(
                colors: [palette.source(tone: 46).opacity(0.62), palette.source(tone: 30).opacity(0.72)],
                startPoint: .topLeading, endPoint: .bottomTrailing
            )
            .animation(.easeInOut(duration: 0.6), value: palette.source)
        }
    }
}

private struct Header: View {
    @Environment(AppModel.self) private var model
    let showSettings: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text("obojeno")
                .font(.title3.weight(.bold))
            Spacer(minLength: 8)

            HStack(spacing: 2) {
                ForEach(Density.allCases, id: \.self) { density in
                    Button {
                        withAnimation(.snappy) { model.density = density }
                    } label: {
                        Image(systemName: density.symbol)
                            .font(.system(size: 12, weight: .semibold))
                            .frame(width: 30, height: 26)
                            .background(
                                Capsule().fill(.white.opacity(model.density == density ? 0.22 : 0)))
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .help("\(density.rawValue.capitalized) tiles")
                }
            }
            .padding(3)
            .background(Capsule().fill(.white.opacity(0.1)))
            .overlay(Capsule().strokeBorder(.white.opacity(0.14)))

            CircleButton(
                symbol: model.mode == .dark ? "moon.fill" : "sun.max.fill",
                help: model.mode == .dark ? "Switch to light" : "Switch to dark",
                action: model.toggleMode)
            CircleButton(symbol: "plus", help: "Add images to your library", action: model.importImages)
            Menu {
                Button("Settings…", action: showSettings)
                    .keyboardShortcut(",")
                Divider()
                Button("Next Wallpaper") { model.step(.next) }
                Button("Previous Wallpaper") { model.step(.previous) }
                Button("Random Wallpaper") { model.step(.random) }
                Divider()
                Button("Quit obojeno") { NSApp.terminate(nil) }
                    .keyboardShortcut("q")
            } label: {
                CircleLabel(symbol: "ellipsis")
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("More")
        }
    }
}

private struct CircleLabel: View {
    let symbol: String

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 12, weight: .bold))
            .frame(width: 32, height: 32)
            .background(Circle().fill(.white.opacity(0.14)))
            .overlay(Circle().strokeBorder(.white.opacity(0.16)))
            .contentShape(Circle())
    }
}

private struct CircleButton: View {
    let symbol: String
    let help: String
    let action: () -> Void

    var body: some View {
        Button(action: action) { CircleLabel(symbol: symbol) }
            .buttonStyle(.plain)
            .help(help)
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
                .buttonStyle(CapsuleButtonStyle())
                .keyboardShortcut(.defaultAction)
            if let error = model.error {
                Text(error).font(.caption).foregroundStyle(.red).multilineTextAlignment(.center)
            }
            Button("Quit") { NSApp.terminate(nil) }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .padding(.top, 6)
        }
        .padding(20)
        .frame(maxWidth: .infinity)
    }
}
