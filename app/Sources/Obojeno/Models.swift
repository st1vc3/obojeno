// The daemon's protocol (see obojeno-core/src/ipc.rs). Coding keys are spelled
// out: `convertFromSnakeCase` would also rewrite palette role names.

import SwiftUI

enum Mode: String, Codable, Sendable, CaseIterable {
    case light, dark
}

enum TransitionStyle: String, Codable, Sendable, CaseIterable {
    case fade
    case growCircle = "grow-circle"
    case wipe
    case none

    var label: String {
        switch self {
        case .fade: "Fade"
        case .growCircle: "Circle"
        case .wipe: "Wipe"
        case .none: "None"
        }
    }
}

enum SchemeVariant: String, Codable, Sendable, CaseIterable {
    case content
    case tonalSpot = "tonal-spot"
    case expressive, fidelity
    case fruitSalad = "fruit-salad"
    case monochrome, neutral, rainbow, vibrant

    var label: String {
        switch self {
        case .tonalSpot: "Tonal Spot"
        case .fruitSalad: "Fruit Salad"
        default: rawValue.capitalized
        }
    }
}

struct Palette: Codable, Sendable, Equatable {
    let source: String
    let variant: SchemeVariant
    let light: [String: String]
    let dark: [String: String]

    func color(_ role: String, _ mode: Mode) -> Color {
        Color(hex: (mode == .dark ? dark : light)[role] ?? source)
    }
}

struct DaemonState: Codable, Sendable, Equatable {
    let wallpaper: String?
    let fixedColor: String?
    let mode: Mode
    let palette: Palette
    let transition: TransitionStyle
    let duration: Double
    let scheme: SchemeVariant
    let contrast: Double
    let wallpaperDirs: [String]

    enum CodingKeys: String, CodingKey {
        case wallpaper, mode, palette, transition, duration, scheme, contrast
        case fixedColor = "fixed_color"
        case wallpaperDirs = "wallpaper_dirs"
    }
}

struct GalleryItem: Codable, Sendable, Equatable, Identifiable {
    let path: String
    let source: String?
    let swatches: [String]
    let thumbnail: String?
    let current: Bool

    var id: String { path }
    var name: String { URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent }
}

struct GalleryFolder: Codable, Sendable, Equatable, Identifiable {
    let path: String
    let items: [GalleryItem]
    let error: String?

    var id: String { path }
    var name: String { URL(fileURLWithPath: path).lastPathComponent }
}

struct Gallery: Codable, Sendable, Equatable {
    let recent: [GalleryItem]
    let folders: [GalleryFolder]
    let pending: Int

    var allItems: [GalleryItem] { recent + folders.flatMap(\.items) }
}

enum Reply: Decodable, Sendable {
    case report
    case state(DaemonState)
    case gallery(Gallery)
    case error(String)

    private enum Keys: String, CodingKey { case reply, message }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .reply) {
        case "report": self = .report
        case "state": self = .state(try DaemonState(from: decoder))
        case "gallery": self = .gallery(try Gallery(from: decoder))
        case "error": self = .error(try c.decode(String.self, forKey: .message))
        case let other:
            throw DecodingError.dataCorruptedError(
                forKey: .reply, in: c, debugDescription: "unknown reply \(other)")
        }
    }
}

struct ConfigChange: Encodable, Sendable {
    var scheme: SchemeVariant?
    var transition: TransitionStyle?
    var duration: Double?
    var wallpaperDirs: [String]?

    enum CodingKeys: String, CodingKey {
        case scheme, transition, duration
        case wallpaperDirs = "wallpaper_dirs"
    }
}

enum Request: Encodable, Sendable {
    case subscribe
    case gallery
    case setWallpaper(path: String)
    case setMode(Mode)
    case configure(ConfigChange)

    private enum Keys: String, CodingKey { case cmd, path, mode, change }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case .subscribe:
            try c.encode("subscribe", forKey: .cmd)
        case .gallery:
            try c.encode("gallery", forKey: .cmd)
        case .setWallpaper(let path):
            try c.encode("set_wallpaper", forKey: .cmd)
            try c.encode(path, forKey: .path)
        case .setMode(let mode):
            try c.encode("set_mode", forKey: .cmd)
            try c.encode(mode, forKey: .mode)
        case .configure(let change):
            try c.encode("configure", forKey: .cmd)
            try c.encode(change, forKey: .change)
        }
    }
}

extension Color {
    /// `#rrggbb`; anything else is gray.
    init(hex: String) {
        let digits = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        let value = digits.count == 6 ? UInt32(digits, radix: 16) ?? 0x808080 : 0x808080
        self.init(
            .sRGB,
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255)
    }
}
