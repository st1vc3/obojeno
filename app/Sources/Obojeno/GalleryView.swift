import AppKit
import SwiftUI

struct GalleryView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            if let gallery = model.gallery {
                LazyVStack(alignment: .leading, spacing: 18) {
                    if !gallery.recent.isEmpty {
                        Section(title: "Recent") {
                            ScrollView(.horizontal, showsIndicators: false) {
                                LazyHStack(spacing: 10) {
                                    ForEach(gallery.recent) { Tile(item: $0, compact: true).frame(width: 128) }
                                }
                                .padding(.vertical, 4)
                                .padding(.horizontal, 14)
                            }
                            // Edge to edge, past the popover's margins.
                            .padding(.horizontal, -14)
                        }
                    }
                    ForEach(gallery.folders) { folder in
                        Section(title: folder.name, help: folder.path) {
                            if let error = folder.error {
                                Label("This folder \(error).", systemImage: "folder.badge.questionmark")
                                    .font(.callout)
                                    .foregroundStyle(.secondary)
                            } else if folder.items.isEmpty {
                                EmptyLibrary(library: folder.library)
                            } else {
                                LazyVGrid(
                                    columns: Array(
                                        repeating: GridItem(.flexible(), spacing: 10), count: model.density.columns),
                                    spacing: 10
                                ) {
                                    ForEach(folder.items) { Tile(item: $0, library: folder.library) }
                                }
                                .padding(.vertical, 4)
                            }
                        }
                    }
                }
                .padding(.bottom, 6)
                // Scrolls (and clips) at the popover's edges, not its margins,
                // so rows and hover lifts are not cut off early.
                .padding(.horizontal, 14)
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 200)
            }
        }
        .scrollIndicators(.never)
        .frame(height: PopoverSize.contentHeight)
        .padding(.horizontal, -14)
    }
}

private struct Section<Content: View>: View {
    let title: String
    var help: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.white.opacity(0.75))
                .help(help ?? title)
            content
        }
    }
}

private struct EmptyLibrary: View {
    @Environment(AppModel.self) private var model
    let library: Bool

    var body: some View {
        if library {
            Button(action: model.importImages) {
                VStack(spacing: 6) {
                    Image(systemName: "photo.badge.plus").font(.system(size: 22))
                    Text("Add images to your library").font(.callout.weight(.medium))
                    Text("They are copied, so they stay even if the originals move.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, minHeight: 120)
                .background(
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .strokeBorder(.white.opacity(0.25), style: StrokeStyle(lineWidth: 1.5, dash: [5, 4])))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } else {
            Text("No images").font(.callout).foregroundStyle(.secondary)
        }
    }
}

/// One wallpaper: the image with its name over a soft shade, ringed when
/// it is the one on screen.
private struct Tile: View {
    @Environment(AppModel.self) private var model
    let item: GalleryItem
    var library = false
    var compact = false
    @ViewState private var hovering = false

    var body: some View {
        let current = model.choosing.map { $0 == item.path } ?? item.current
        let showName = !compact && model.density != .small
        Button {
            model.setWallpaper(item.path)
        } label: {
            Thumbnail(path: item.thumbnail, fallback: .white.opacity(0.12))
                .aspectRatio(16 / 10, contentMode: .fit)
                .overlay(alignment: .bottomLeading) {
                    if showName {
                        Text(item.name)
                            .font(.system(size: model.density == .large ? 13 : 11, weight: .semibold))
                            .foregroundStyle(.white)
                            .lineLimit(1)
                            .shadow(color: .black.opacity(0.5), radius: 3, y: 1)
                            .padding(.horizontal, 9)
                            .padding(.vertical, 7)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(
                                LinearGradient(colors: [.clear, .black.opacity(0.45)], startPoint: .top, endPoint: .bottom))
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(.white.opacity(current ? 0.95 : hovering ? 0.4 : 0.12), lineWidth: current ? 2.5 : 1)
                )
                .shadow(color: .black.opacity(hovering ? 0.35 : 0.22), radius: hovering ? 9 : 5, y: hovering ? 5 : 3)
                .scaleEffect(hovering ? 1.035 : 1)
                .animation(.snappy(duration: 0.18), value: hovering)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(item.name)
        .accessibilityLabel(item.name)
        .contextMenu {
            Button("Set as Wallpaper") { model.setWallpaper(item.path) }
            Button("Show in Finder") { model.showInFinder(item) }
            if library {
                Divider()
                Button("Move to Trash", role: .destructive) { model.removeFromLibrary(item) }
            }
        }
    }
}

/// A thumbnail file from the daemon's cache, loaded off the main thread.
struct Thumbnail: View {
    let path: String?
    let fallback: Color
    @ViewState private var image: NSImage?

    /// Fills whatever frame it gets: the image is overlaid on the fallback
    /// color (which takes the proposed size) and clipped to it, so wide
    /// images never spill out.
    var body: some View {
        fallback
            .overlay {
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFill()
                        .transition(.opacity)
                }
            }
            .clipped()
            .task(id: path) {
                guard let path else {
                    image = nil
                    return
                }
                if let cached = ThumbnailCache.shared.object(forKey: path as NSString) {
                    image = cached
                    return
                }
                let loaded = await Task.detached(priority: .userInitiated) { NSImage(contentsOfFile: path) }.value
                if let loaded { ThumbnailCache.shared.setObject(loaded, forKey: path as NSString) }
                withAnimation(.easeOut(duration: 0.2)) { image = loaded }
            }
    }
}

enum ThumbnailCache {
    nonisolated(unsafe) static let shared: NSCache<NSString, NSImage> = {
        let cache = NSCache<NSString, NSImage>()
        cache.countLimit = 300
        return cache
    }()
}
