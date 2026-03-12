import AppKit
import Foundation

guard CommandLine.arguments.count >= 3 else {
    fputs("Usage: make-dmg-background.swift <output-path> <app-name>\n", stderr)
    exit(1)
}

let outputPath = CommandLine.arguments[1]
let appName = CommandLine.arguments[2]
let canvasSize = NSSize(width: 720, height: 420)
let canvasRect = NSRect(origin: .zero, size: canvasSize)

let image = NSImage(size: canvasSize)
image.lockFocus()

guard let context = NSGraphicsContext.current?.cgContext else {
    fputs("Could not create drawing context.\n", stderr)
    exit(1)
}

func color(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1.0) -> NSColor {
    NSColor(calibratedRed: r / 255.0, green: g / 255.0, blue: b / 255.0, alpha: a)
}

let gradient = NSGradient(colors: [
    color(7, 12, 21),
    color(10, 22, 39),
    color(12, 31, 54),
])!
gradient.draw(in: canvasRect, angle: -90)

context.saveGState()
context.setFillColor(color(88, 208, 255, 0.08).cgColor)
context.fillEllipse(in: CGRect(x: -40, y: 260, width: 220, height: 140))
context.setFillColor(color(106, 255, 213, 0.07).cgColor)
context.fillEllipse(in: CGRect(x: 535, y: 36, width: 170, height: 170))
context.restoreGState()

let leftCard = NSBezierPath(roundedRect: NSRect(x: 94, y: 112, width: 176, height: 168), xRadius: 24, yRadius: 24)
color(255, 255, 255, 0.05).setFill()
leftCard.fill()
color(255, 255, 255, 0.10).setStroke()
leftCard.lineWidth = 1.0
leftCard.stroke()

let rightCard = NSBezierPath(roundedRect: NSRect(x: 450, y: 112, width: 176, height: 168), xRadius: 24, yRadius: 24)
color(255, 255, 255, 0.05).setFill()
rightCard.fill()
color(255, 255, 255, 0.10).setStroke()
rightCard.lineWidth = 1.0
rightCard.stroke()

let divider = NSBezierPath()
divider.move(to: NSPoint(x: 310, y: 196))
divider.line(to: NSPoint(x: 410, y: 196))
color(255, 255, 255, 0.14).setStroke()
divider.lineWidth = 2.0
divider.lineCapStyle = .round
divider.stroke()

let titleStyle = NSMutableParagraphStyle()
titleStyle.alignment = .left

let titleAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 28, weight: .bold),
    .foregroundColor: color(250, 252, 255),
    .paragraphStyle: titleStyle,
]

let subtitleAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 15, weight: .medium),
    .foregroundColor: color(206, 220, 244, 0.88),
    .paragraphStyle: titleStyle,
]

let captionStyle = NSMutableParagraphStyle()
captionStyle.alignment = .center

let captionAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 14, weight: .semibold),
    .foregroundColor: color(222, 232, 248, 0.92),
    .paragraphStyle: captionStyle,
]

let footerAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 12, weight: .medium),
    .foregroundColor: color(188, 202, 228, 0.78),
    .paragraphStyle: captionStyle,
]

let arrowAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 76, weight: .bold),
    .foregroundColor: color(255, 255, 255, 0.32),
    .paragraphStyle: captionStyle,
]

(appName as NSString).draw(in: NSRect(x: 54, y: 336, width: 320, height: 36), withAttributes: titleAttrs)
("Drag into Applications to install" as NSString).draw(
    in: NSRect(x: 54, y: 315, width: 300, height: 24),
    withAttributes: subtitleAttrs
)

("→" as NSString).draw(
    in: NSRect(x: 302, y: 150, width: 116, height: 88),
    withAttributes: arrowAttrs
)
("Install" as NSString).draw(
    in: NSRect(x: 286, y: 132, width: 148, height: 22),
    withAttributes: captionAttrs
)
("Open it from Applications after copying." as NSString).draw(
    in: NSRect(x: 184, y: 38, width: 352, height: 20),
    withAttributes: footerAttrs
)

image.unlockFocus()

guard
    let tiffData = image.tiffRepresentation,
    let bitmap = NSBitmapImageRep(data: tiffData),
    let pngData = bitmap.representation(using: .png, properties: [:])
else {
    fputs("Could not encode background image.\n", stderr)
    exit(1)
}

let outputURL = URL(fileURLWithPath: outputPath)
try FileManager.default.createDirectory(
    at: outputURL.deletingLastPathComponent(),
    withIntermediateDirectories: true,
    attributes: nil
)
try pngData.write(to: outputURL)
