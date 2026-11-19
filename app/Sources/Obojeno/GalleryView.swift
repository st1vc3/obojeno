import AppKit
import SwiftUI

struct GalleryView: View {
    @Environment(AppModel.self) private var model

    private let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 3)

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                if let gallery = model.gallery {
                    if !gallery.recent.isEmpty {
                        Section(title: "Recent") {
                            ScrollView(.horizontal, showsIndicators: false) {
                                LazyHStack(spacing: 8) {
                                    ForEach(gallery.recent) { item in
                                        Tile(item: item).frame(width: 104)
                                    }
                                }
                                .padding(2)
                            }
                        }
                    }
                    ForEach(gallery.folders) { folder in
                        Section(title: folder.name, help: folder.path) {
                            if let error = folder.error {
                                Text("Can't read this folder: \(error)")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            } else if folder.items.isEmpty {
                                Text("No images").font(.caption).foregroundStyle(.secondary)
                            } else {
                                LazyVGrid(columns: columns, spacing: 8) {
                                    ForEach(folder.items) { Tile(item: $0) }
                                }
                                .padding(2)
                            }
                        }
                    }
                } else {
                    ProgressView().frame(maxWidth: .infinity, minHeight: 120)
                }
            }
        }
        .frame(height: 360)
    }
}

private struct Section<Content: View>: View {
    let title: String
    var help: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .help(help ?? title)
            content
        }
    }
}

/// One wallpaper: thumbnail, its theme's swatches, a ring when current.
private struct Tile: View {
    @Environment(AppModel.self) private var model
    let item: GalleryItem
    @ViewState private var hovering = false

    var body: some View {
        let current = model.choosing.map { $0 == item.path } ?? item.current
        Button {
            model.setWallpaper(item.path)
        } label: {
            VStack(spacing: 0) {
                Thumbnail(path: item.thumbnail, fallback: Color.secondary.opacity(0.15))
                    .aspectRatio(16 / 10, contentMode: .fill)
                    .clipped()
                HStack(spacing: 0) {
                    if item.swatches.isEmpty {
                        Color.secondary.opacity(0.2)
                    } else {
                        ForEach(Array(item.swatches.enumerated()), id: \.offset) { Color(hex: $0.element) }
                    }
                }
                .frame(height: 5)
            }
            .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(current ? model.color("primary") : .primary.opacity(hovering ? 0.35 : 0.1),
                                  lineWidth: current ? 2.5 : 1))
            .scaleEffect(hovering ? 1.03 : 1)
            .animation(.snappy(duration: 0.15), value: hovering)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(item.name)
        .accessibilityLabel(item.name)
    }
}

/// A thumbnail file from the daemon's cache, loaded off the main thread.
struct Thumbnail: View {
    let path: String?
    let fallback: Color
    @ViewState private var image: NSImage?

    var body: some View {
        ZStack {
            fallback
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFill()
                    .transition(.opacity)
            }
        }
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
