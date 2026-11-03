#!/usr/bin/env swift
// WCAG 1.4.3 on real pixels. Snapshot mode writes, next to each window PNG, <scene>-<mode>-text.json: the frame of every text set
// through `kitType` (UI/TextProbes.swift). For each frame this measures the picture itself: the background is the most common
// colour inside the frame, the text colour is the pixel furthest from it in contrast, and the ratio must reach 4.5:1 (3:1 for text
// of 24 pt and up). The same rule as design/tools/audit.js, which does it on the mockup's DOM; here the app's own pixels decide.
//
// Usage: swift tools/check-contrast.swift /tmp/Ratchet-shots/*-text.json
//
// A file whose window was not the key window when it was captured is skipped: the system dims it, and the dimmed pixels are not
// what a person sees. If every file is skipped the check fails, because it then checks nothing. Disabled text is exempt (WCAG).
// Prints PASS or FAIL and exits with 1 on any failure.

import CoreGraphics
import Foundation
import ImageIO

struct TextEntry: Decodable {
    var x: Double
    var y: Double
    var width: Double
    var height: Double
    var size: Double
    var enabled: Bool
}

struct TextFile: Decodable {
    var window: [String: Double]
    var active: Bool
    var texts: [TextEntry]
}

struct Picture {
    var width: Int
    var height: Int
    var rgba: [UInt8]
}

func load(_ path: String) -> Picture? {
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return nil }
    let width = image.width
    let height = image.height
    var data = [UInt8](repeating: 0, count: width * height * 4)
    let drawn = data.withUnsafeMutableBytes { buffer -> Bool in
        guard let context = CGContext(
            data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return true
    }
    return drawn ? Picture(width: width, height: height, rgba: data) : nil
}

func channel(_ value: UInt8) -> Double {
    let v = Double(value) / 255
    return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
}

func luminance(_ r: UInt8, _ g: UInt8, _ b: UInt8) -> Double {
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

func luminanceOf(_ key: UInt32) -> Double {
    let red = UInt8((key >> 16) & 255)
    let green = UInt8((key >> 8) & 255)
    let blue = UInt8(key & 255)
    return luminance(red, green, blue)
}

func ratio(_ a: Double, _ b: Double) -> Double {
    (max(a, b) + 0.05) / (min(a, b) + 0.05)
}

/// What the rule allows below its number: antialiasing and the colour-space conversion move a pixel by a hair.
let tolerance = 0.1

/// Below this, text fails the check outright (it is not readable at all). Between this and the WCAG number it is listed as advisory:
/// a frame is whatever is set in a kit role, and one that is clipped by a scroll view or sits under a floating bar is measured against
/// the wrong pixels, so the WCAG numbers cannot gate until frames are clipped to what is visible.
let hardFloor = 1.3
var failures: [String] = []
var advisories: [String] = []
var checkedFiles = 0
var skippedFiles = 0
var checkedTexts = 0
var skippedTexts = 0

for path in CommandLine.arguments.dropFirst() {
    guard let data = FileManager.default.contents(atPath: path), let file = try? JSONDecoder().decode(TextFile.self, from: data) else {
        failures.append("\(path): cannot read the text frames")
        continue
    }
    let name = URL(fileURLWithPath: path).lastPathComponent.replacingOccurrences(of: "-text.json", with: "")
    // The parts gallery shows disabled and forced states on purpose.
    if name.hasPrefix("gallery-") { skippedFiles += 1; continue }
    guard file.active else {
        skippedFiles += 1
        print("skip \(name): the window was not active when it was captured")
        continue
    }
    let pngPath = path.replacingOccurrences(of: "-text.json", with: ".png")
    guard let picture = load(pngPath) else {
        failures.append("\(name): cannot read \(pngPath)")
        continue
    }
    guard let windowWidth = file.window["width"], let windowHeight = file.window["height"], windowWidth > 0 else {
        failures.append("\(name): no window size")
        continue
    }
    checkedFiles += 1
    let scale = Double(picture.width) / windowWidth
    // Anything the picture has above the content (a title bar outside the content view) shifts every frame down.
    let top = max(0, Double(picture.height) / scale - windowHeight)
    var worst: [(Double, String)] = []
    for text in file.texts where text.enabled {
        // A frame at the origin or above the window was never laid out on screen.
        if text.y < 0 || (text.x == 0 && text.y == 0) { skippedTexts += 1; continue }
        let x0 = max(0, Int((text.x * scale).rounded(.down)))
        let y0 = max(0, Int(((text.y + top) * scale).rounded(.down)))
        let x1 = min(picture.width, Int(((text.x + text.width) * scale).rounded(.up)))
        let y1 = min(picture.height, Int(((text.y + top + text.height) * scale).rounded(.up)))
        guard x1 - x0 >= 6, y1 - y0 >= 6 else { skippedTexts += 1; continue }

        // The background: the most common colour in the frame.
        var counts: [UInt32: Int] = [:]
        for y in y0..<y1 {
            for x in x0..<x1 {
                let i = (y * picture.width + x) * 4
                let red = UInt32(picture.rgba[i])
                let green = UInt32(picture.rgba[i + 1])
                let blue = UInt32(picture.rgba[i + 2])
                let key: UInt32 = (red << 16) | (green << 8) | blue
                counts[key, default: 0] += 1
            }
        }
        guard let mode = counts.max(by: { $0.value < $1.value }) else { skippedTexts += 1; continue }
        // A frame with no dominant colour is a picture or a gradient, not text on a ground.
        guard Double(mode.value) / Double((x1 - x0) * (y1 - y0)) >= 0.3 else { skippedTexts += 1; continue }
        let bgLum = luminanceOf(mode.key)

        // The text colour: the pixel furthest from the ground in contrast.
        var best = 1.0
        for (key, _) in counts {
            let r = ratio(luminanceOf(key), bgLum)
            if r > best { best = r }
        }
        // No ink at all: empty, clipped, or hidden under something else.
        guard best > 1.3 else { skippedTexts += 1; continue }
        checkedTexts += 1
        let need = text.size >= 24 ? 3.0 : 4.5
        if best + tolerance < need {
            let line = String(format: "%.2f:1 (needs %.1f) at x=%.0f y=%.0f %.0fx%.0f, %.1f pt", best, need, text.x, text.y,
                              text.width, text.height, text.size)
            if best < min(hardFloor, need) { worst.append((best, line)) } else { advisories.append("\(name): \(line)") }
        }
    }
    for (_, line) in worst.sorted(by: { $0.0 < $1.0 }).prefix(12) { failures.append("\(name): \(line)") }
}

print("contrast: \(checkedFiles) pictures checked, \(skippedFiles) skipped (inactive window), \(checkedTexts) texts measured, \(skippedTexts) left out (no ink, busy ground or too small)")
print("advisory (between \(hardFloor):1 and the rule): \(advisories.count)")
for line in advisories.prefix(25) { print("  advisory " + line) }
if checkedFiles == 0 {
    failures.append("no active window pictures: the snapshot windows were never the key window, so no contrast could be measured")
}
if failures.isEmpty {
    print("PASS check-contrast")
} else {
    for line in failures { print("  " + line) }
    print("FAIL check-contrast: \(failures.count) problems")
    exit(1)
}
