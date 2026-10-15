// Deterministic AppKit renderer for assets/app-icon.svg's simple vector mark.
// Run from the repository root: swift scripts/render_app_icon.swift
import AppKit
import Foundation

func iconPNG(_ side: Int) -> Data {
    let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
        isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0
    )!
    let context = NSGraphicsContext(bitmapImageRep: bitmap)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    let cg = context.cgContext
    cg.setAllowsAntialiasing(true)
    cg.setShouldAntialias(true)
    cg.scaleBy(x: CGFloat(side) / 1024, y: CGFloat(side) / 1024)
    cg.setFillColor(NSColor(srgbRed: 50/255, green: 104/255, blue: 233/255, alpha: 1).cgColor)
    cg.addPath(CGPath(roundedRect: CGRect(x: 32, y: 32, width: 960, height: 960),
                      cornerWidth: 212, cornerHeight: 212, transform: nil))
    cg.fillPath()
    cg.setStrokeColor(NSColor.white.cgColor)
    cg.setLineWidth(88)
    cg.setLineCap(.round)
    cg.setLineJoin(.round)
    func stroke(_ points: [CGPoint]) {
        cg.addLines(between: points)
        cg.strokePath()
    }
    stroke([CGPoint(x: 256, y: 348), CGPoint(x: 426, y: 348), CGPoint(x: 572, y: 512)])
    stroke([CGPoint(x: 256, y: 676), CGPoint(x: 426, y: 676), CGPoint(x: 572, y: 512)])
    stroke([CGPoint(x: 572, y: 512), CGPoint(x: 766, y: 512)])
    stroke([CGPoint(x: 676, y: 422), CGPoint(x: 766, y: 512), CGPoint(x: 676, y: 602)])
    context.flushGraphics()
    NSGraphicsContext.restoreGraphicsState()
    return bitmap.representation(using: .png, properties: [:])!
}

extension Data {
    mutating func appendLE(_ value: UInt16) {
        append(UInt8(value & 0xff)); append(UInt8(value >> 8))
    }
    mutating func appendLE(_ value: UInt32) {
        for shift in [0, 8, 16, 24] { append(UInt8((value >> shift) & 0xff)) }
    }
}

let fm = FileManager.default
let assets = URL(fileURLWithPath: "assets", isDirectory: true)
try iconPNG(1024).write(to: assets.appendingPathComponent("app-icon.png"))

let iconset = assets.appendingPathComponent("app-icon.iconset", isDirectory: true)
try? fm.removeItem(at: iconset)
try fm.createDirectory(at: iconset, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    try iconPNG(size).write(to: iconset.appendingPathComponent("icon_\(size)x\(size).png"))
    try iconPNG(size * 2).write(to: iconset.appendingPathComponent("icon_\(size)x\(size)@2x.png"))
}

let sizes = [16, 32, 48, 64, 128, 256]
let images = sizes.map(iconPNG)
var ico = Data()
ico.appendLE(UInt16(0)); ico.appendLE(UInt16(1)); ico.appendLE(UInt16(sizes.count))
var offset = UInt32(6 + sizes.count * 16)
for (size, image) in zip(sizes, images) {
    ico.append(UInt8(size == 256 ? 0 : size)); ico.append(UInt8(size == 256 ? 0 : size))
    ico.append(UInt8(0)); ico.append(UInt8(0))
    ico.appendLE(UInt16(1)); ico.appendLE(UInt16(32))
    ico.appendLE(UInt32(image.count)); ico.appendLE(offset)
    offset += UInt32(image.count)
}
for image in images { ico.append(image) }
try ico.write(to: assets.appendingPathComponent("app-icon.ico"))

let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", assets.appendingPathComponent("app-icon.icns").path]
try iconutil.run()
iconutil.waitUntilExit()
guard iconutil.terminationStatus == 0 else { fatalError("iconutil failed") }
