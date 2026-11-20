// Draws obojeno's app icon: a fan of color swatches, one palette in tones,
// as obojeno makes one from a wallpaper. Run by scripts/build-app.sh:
//
//   swift app/Icon/icon.swift OUT.png [SIZE]
//
// The artwork follows macOS's icon grid: a rounded square 824/1024 of the
// canvas, centered, with room for its shadow.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    FileHandle.standardError.write("usage: icon.swift OUT.png [SIZE]\n".data(using: .utf8)!)
    exit(2)
}
let output = URL(fileURLWithPath: arguments[1])
let size = arguments.count > 2 ? Int(arguments[2]) ?? 1024 : 1024

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
        blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

/// A superellipse, |x|^n + |y|^n = 1, which is what Apple's icon shape
/// closely follows: corners that curve continuously into the sides.
func squircle(_ rect: CGRect, exponent n: CGFloat = 5) -> CGPath {
    let path = CGMutablePath()
    let (a, b) = (rect.width / 2, rect.height / 2)
    let steps = 720
    for step in 0..<steps {
        let t = CGFloat(step) / CGFloat(steps) * 2 * .pi
        let (c, s) = (cos(t), sin(t))
        let point = CGPoint(
            x: rect.midX + a * copysign(pow(abs(c), 2 / n), c),
            y: rect.midY + b * copysign(pow(abs(s), 2 / n), s))
        step == 0 ? path.move(to: point) : path.addLine(to: point)
    }
    path.closeSubpath()
    return path
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(
    data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
// Drawn in 1024 units.
context.scaleBy(x: CGFloat(size) / 1024, y: CGFloat(size) / 1024)

let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let shape = squircle(tile)

// The shadow under the tile.
context.saveGState()
context.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: color(0x000000, 0.35))
context.addPath(shape)
context.setFillColor(color(0x101014))
context.fillPath()
context.restoreGState()

context.saveGState()
context.addPath(shape)
context.clip()

// Background: a deep, slightly violet night.
let background = CGGradient(
    colorsSpace: space, colors: [color(0x2A2233), color(0x0E0C12)] as CFArray, locations: [0, 1])!
context.drawLinearGradient(
    background, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])

// The fan: blades pivot near the bottom left corner and open up to the
// top right. From the back (deep plum) to the front (light peach): tones
// of one warm palette.
let pivot = CGPoint(x: 292, y: 292)
let blades: [(angle: CGFloat, base: UInt32, tip: UInt32)] = [
    (80, 0x5E1F7A, 0x8A3BA8),
    (64, 0x9C1F5E, 0xC93C80),
    (48, 0xD4304E, 0xF2566B),
    (32, 0xF0613A, 0xFF8A5C),
    (16, 0xFF9E6E, 0xFFC4A0),
]
let length: CGFloat = 575
let width: CGFloat = 158

for blade in blades {
    context.saveGState()
    context.translateBy(x: pivot.x, y: pivot.y)
    context.rotate(by: blade.angle * .pi / 180)
    // Along +x from the pivot, centered on the axis.
    let rect = CGRect(x: -width / 2, y: -width / 2, width: length + width / 2, height: width)
    let path = CGPath(roundedRect: rect, cornerWidth: width * 0.42, cornerHeight: width * 0.42, transform: nil)

    // Each blade casts a soft shadow onto the ones behind it.
    context.saveGState()
    context.setShadow(offset: CGSize(width: 0, height: -14), blur: 30, color: color(0x000000, 0.45))
    context.addPath(path)
    context.setFillColor(color(blade.base))
    context.fillPath()
    context.restoreGState()

    // Lighter towards the tip, like light falling across the fan.
    context.saveGState()
    context.addPath(path)
    context.clip()
    let gradient = CGGradient(
        colorsSpace: space, colors: [color(blade.base), color(blade.tip)] as CFArray, locations: [0, 1])!
    context.drawLinearGradient(
        gradient, start: CGPoint(x: 0, y: 0), end: CGPoint(x: length, y: 0), options: [])
    // A thin highlight along the upper edge.
    let edge = CGGradient(
        colorsSpace: space, colors: [color(0xFFFFFF, 0.22), color(0xFFFFFF, 0)] as CFArray, locations: [0, 1])!
    context.drawLinearGradient(
        edge, start: CGPoint(x: 0, y: width / 2), end: CGPoint(x: 0, y: width / 2 - 26), options: [])
    context.restoreGState()
    context.restoreGState()
}

// The rivet the blades turn on.
context.saveGState()
context.setShadow(offset: CGSize(width: 0, height: -4), blur: 10, color: color(0x000000, 0.5))
context.addEllipse(in: CGRect(x: pivot.x - 30, y: pivot.y - 30, width: 60, height: 60))
context.setFillColor(color(0xF4EEF7))
context.fillPath()
context.restoreGState()

// A soft sheen across the top of the tile.
let sheen = CGGradient(
    colorsSpace: space, colors: [color(0xFFFFFF, 0.10), color(0xFFFFFF, 0)] as CFArray, locations: [0, 1])!
context.drawLinearGradient(sheen, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 640), options: [])
context.restoreGState()

// A hairline rim, as on Apple's icons.
context.addPath(shape)
context.setStrokeColor(color(0xFFFFFF, 0.08))
context.setLineWidth(2)
context.strokePath()

let image = context.makeImage()!
let destination = CGImageDestinationCreateWithURL(output as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(destination, image, nil)
guard CGImageDestinationFinalize(destination) else {
    FileHandle.standardError.write("cannot write \(output.path)\n".data(using: .utf8)!)
    exit(1)
}
