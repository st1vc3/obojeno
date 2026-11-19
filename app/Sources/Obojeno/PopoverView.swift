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
                if model.needsWelcome {
                    Page { WelcomeView() }
                } else if showingSettings {
                    Page { SettingsView { withAnimation(.snappy) { showingSettings = false } } }
                } else {
                    Header(showSettings: { withAnimation(.snappy) { showingSettings = true } })
                    UpdateBanner()
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
        .environment(\.colorScheme, .dark)
        .tint(model.accent)
    }
}

/// A full-height page in place of the header and gallery, such as the
/// settings.
private struct Page<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView {
            content.padding(.bottom, 4)
        }
        .scrollIndicators(.never)
        // As tall as the header and gallery together.
        .frame(height: PopoverSize.contentHeight + 44)
    }
}

private struct Header: View {
    @Environment(AppModel.self) private var model
    let showSettings: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text("obojeno")
                .font(.title3.weight(.bold))
                .legibleOnGlass()
            Spacer(minLength: 8)
            // Neighboring glass shapes blend into each other.
            GlassEffectContainer(spacing: 8) {
                HStack(spacing: 8) { controls }
            }
        }
    }

    @ViewBuilder private var controls: some View {

        HStack(spacing: 2) {
            ForEach(Density.allCases, id: \.self) { density in
                Button {
                    withAnimation(.snappy) { model.density = density }
                } label: {
                    Image(systemName: density.symbol)
                        .font(.system(size: 12, weight: .semibold))
                        .frame(width: 30, height: 26)
                        .background(
                            Capsule().fill(.white.opacity(model.density == density ? 0.22 : 0))
                        )
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .help("\(density.rawValue.capitalized) tiles")
            }
        }
        .padding(3)
        .glassEffect(.regular, in: .capsule)

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
            Button("Check for Updates…") {
                Task { await model.updater.check(userInitiated: true) }
            }
            .disabled(model.updater.checking)
            Button("Show Log") { model.showLog() }
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

/// A newer release, or how a check the user asked for went.
private struct UpdateBanner: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let updater = model.updater
        if let release = updater.available {
            Banner(symbol: "arrow.down.circle.fill", dismiss: updater.dismiss) {
                Text("obojeno \(release.version) is available").font(.callout.weight(.semibold))
                Text(hint).font(.caption).foregroundStyle(.secondary)
            } actions: {
                switch InstallMethod.current {
                case .homebrew:
                    Button("Copy Command") { model.copy(Updater.brewCommand) }
                        .help(Updater.brewCommand)
                case .nix:
                    Button("Release Notes") { NSWorkspace.shared.open(release.page) }
                case .download:
                    Button("Download") { NSWorkspace.shared.open(release.page) }
                }
            }
        } else if let message = updater.message {
            Banner(symbol: "checkmark.circle.fill", dismiss: updater.dismiss) {
                Text(message).font(.callout)
            } actions: {
                EmptyView()
            }
        }
    }

    private var hint: String {
        switch InstallMethod.current {
        case .homebrew: "Update with: \(Updater.brewCommand)"
        case .nix: "Update the obojeno flake input and rebuild."
        case .download: "Download it from GitHub."
        }
    }
}

private struct Banner<Message: View, Actions: View>: View {
    let symbol: String
    let dismiss: () -> Void
    @ViewBuilder let text: Message
    @ViewBuilder let actions: Actions

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol).font(.title3)
            VStack(alignment: .leading, spacing: 2) { text }
            Spacer(minLength: 8)
            actions.buttonStyle(CapsuleButtonStyle())
            Button(action: dismiss) {
                Image(systemName: "xmark").font(.system(size: 11, weight: .bold))
            }
            .buttonStyle(.plain)
            .opacity(0.7)
            .help("Dismiss")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: .rect(cornerRadius: 16))
        .transition(.opacity.combined(with: .move(edge: .top)))
    }
}

private struct CircleLabel: View {
    let symbol: String

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 12, weight: .bold))
            .frame(width: 32, height: 32)
            .contentShape(Circle())
            .glassEffect(.regular.interactive(), in: .circle)
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
