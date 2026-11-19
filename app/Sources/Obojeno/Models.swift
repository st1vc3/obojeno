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

/// The parts of the daemon's palette the app shows.
struct Palette: Codable, Sendable, Equatable {
    let source: String
    let variant: SchemeVariant
    /// Tonal palettes, 101 colors each, indexed by tone (0 black, 100 white).
    let palettes: [String: [String]]

    /// The wallpaper's own color at a tone, at full saturation.
    func source(tone: Int) -> Color {
        Color(hex: palettes["source"].flatMap { $0.indices.contains(tone) ? $0[tone] : nil } ?? source)
    }
}

struct Finish: Codable, Sendable, Equatable {
    /// 0 (off) to 1.
    let grain: Double
    let cornerRadius: Double

    enum CodingKeys: String, CodingKey {
        case grain
        case cornerRadius = "corner_radius"
    }
}

struct DaemonState: Codable, Sendable, Equatable {
    /// Missing from daemons older than 0.1.2.
    let version: String?
    /// False until a config file exists: the first run.
    let configured: Bool?
    let wallpaper: String?
    let fixedColor: String?
    let mode: Mode
    let palette: Palette
    let transition: TransitionStyle
    let duration: Double
    let scheme: SchemeVariant
    let contrast: Double
    let wallpaperDirs: [String]
    let allSpaces: Bool
    let lidAnimation: Bool
    let finish: Finish

    enum CodingKeys: String, CodingKey {
        case version, configured, wallpaper, mode, palette, transition, duration, scheme, contrast, finish
        case fixedColor = "fixed_color"
        case wallpaperDirs = "wallpaper_dirs"
        case allSpaces = "all_spaces"
        case lidAnimation = "lid_animation"
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
    /// obojeno's own library, where imported images go.
    let library: Bool
    let items: [GalleryItem]
    let error: String?

    var id: String { path }
    var name: String { library ? "Library" : URL(fileURLWithPath: path).lastPathComponent }
}

struct Gallery: Codable, Sendable, Equatable {
    let recent: [GalleryItem]
    let folders: [GalleryFolder]
    let pending: Int

}

/// A built-in target (an app obojeno themes), as found on this Mac.
struct TargetInfo: Codable, Sendable, Equatable, Identifiable {
    let name: String
    let title: String
    let enabled: Bool
    let installed: Bool
    /// Whether the one-time setup is done, when obojeno can tell.
    let setUp: Bool?
    let setup: String
    let snippet: String?
    let options: [String: String]
    /// A required option without a value; the target cannot be turned on.
    let missing: String?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, title, enabled, installed, setup, snippet, options, missing
        case setUp = "set_up"
    }
}

enum Reply: Decodable, Sendable {
    case report
    case state(DaemonState)
    case gallery(Gallery)
    case targets([TargetInfo])
    case error(String)

    private enum Keys: String, CodingKey { case reply, message, targets }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .reply) {
        case "report": self = .report
        case "state": self = .state(try DaemonState(from: decoder))
        case "gallery": self = .gallery(try Gallery(from: decoder))
        case "targets": self = .targets(try c.decode([TargetInfo].self, forKey: .targets))
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
    var allSpaces: Bool?
    var lidAnimation: Bool?
    var grain: Double?
    var cornerRadius: Double?
    var targets: [String: TargetChange]?

    enum CodingKeys: String, CodingKey {
        case scheme, transition, duration, grain, targets
        case wallpaperDirs = "wallpaper_dirs"
        case allSpaces = "all_spaces"
        case lidAnimation = "lid_animation"
        case cornerRadius = "corner_radius"
    }
}

struct TargetChange: Encodable, Sendable {
    var enabled: Bool
    var options: [String: String] = [:]
}

enum Direction: String, Encodable, Sendable {
    case next, previous, random
}

enum Request: Encodable, Sendable {
    case subscribe
    case gallery
    case targets
    case setWallpaper(path: String, origin: CGPoint?)
    case setMode(Mode)
    case configure(ConfigChange)
    case importImages([String])
    case remove(String)
    case step(Direction)

    private enum Keys: String, CodingKey { case cmd, path, paths, mode, change, origin, direction }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case .subscribe:
            try c.encode("subscribe", forKey: .cmd)
        case .gallery:
            try c.encode("gallery", forKey: .cmd)
        case .targets:
            try c.encode("targets", forKey: .cmd)
        case .setWallpaper(let path, let origin):
            try c.encode("set_wallpaper", forKey: .cmd)
            try c.encode(path, forKey: .path)
            if let origin { try c.encode([origin.x, origin.y], forKey: .origin) }
        case .setMode(let mode):
            try c.encode("set_mode", forKey: .cmd)
            try c.encode(mode, forKey: .mode)
        case .configure(let change):
            try c.encode("configure", forKey: .cmd)
            try c.encode(change, forKey: .change)
        case .importImages(let paths):
            try c.encode("import", forKey: .cmd)
            try c.encode(paths, forKey: .paths)
        case .remove(let path):
            try c.encode("remove", forKey: .cmd)
            try c.encode(path, forKey: .path)
        case .step(let direction):
            try c.encode("step", forKey: .cmd)
            try c.encode(direction, forKey: .direction)
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
