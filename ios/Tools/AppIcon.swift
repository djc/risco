// Draws the app icon into a new asset catalog, so the repository needs no image files
//
// Usage: swift AppIcon.swift <path to .xcassets>

import AppKit
import UniformTypeIdentifiers

let catalog = URL(fileURLWithPath: CommandLine.arguments[1])
let iconSet = catalog.appending(path: "AppIcon.appiconset")
try FileManager.default.createDirectory(at: iconSet, withIntermediateDirectories: true)

// App icons must be opaque, and iOS rounds the corners itself
let size = 1024
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(
    data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0, space: space,
    bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
)!

let colors = [
    CGColor(srgbRed: 0.42, green: 0.40, blue: 0.98, alpha: 1),
    CGColor(srgbRed: 0.17, green: 0.13, blue: 0.55, alpha: 1),
]
let gradient = CGGradient(colorsSpace: space, colors: colors as CFArray, locations: [0, 1])!
context.drawLinearGradient(
    gradient, start: CGPoint(x: 0, y: size), end: CGPoint(x: size, y: 0), options: []
)

// The lock in the background's darker color, on a white shield
let lock = NSColor(srgbRed: 0.24, green: 0.20, blue: 0.70, alpha: 1)
let configuration = NSImage.SymbolConfiguration(pointSize: 600, weight: .semibold)
    .applying(NSImage.SymbolConfiguration(paletteColors: [lock, .white]))
let symbol = NSImage(systemSymbolName: "lock.shield.fill", accessibilityDescription: nil)!
    .withSymbolConfiguration(configuration)!
let origin = CGPoint(
    x: (CGFloat(size) - symbol.size.width) / 2,
    y: (CGFloat(size) - symbol.size.height) / 2
)
NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
symbol.draw(in: CGRect(origin: origin, size: symbol.size))
NSGraphicsContext.current = nil

let png = iconSet.appending(path: "icon.png")
let destination = CGImageDestinationCreateWithURL(
    png as CFURL, UTType.png.identifier as CFString, 1, nil
)!
CGImageDestinationAddImage(destination, context.makeImage()!, nil)
guard CGImageDestinationFinalize(destination) else { fatalError("failed to write \(png.path)") }

let info = #"{ "info": { "author": "xcode", "version": 1 } }"#
try info.write(to: catalog.appending(path: "Contents.json"), atomically: true, encoding: .utf8)
let contents = """
    {
      "images": [
        { "filename": "icon.png", "idiom": "universal", "platform": "ios", "size": "1024x1024" }
      ],
      "info": { "author": "xcode", "version": 1 }
    }
    """
try contents.write(to: iconSet.appending(path: "Contents.json"), atomically: true, encoding: .utf8)
