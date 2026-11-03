import Foundation
import CoreText
import CoreGraphics

let font = CTFontCreateWithName("HelveticaNeue" as CFString, 100, nil)
let attributes: [NSAttributedString.Key: Any] = [
    NSAttributedString.Key(kCTFontAttributeName as String): font,
    NSAttributedString.Key(kCTKernAttributeName as String): -1.2
]
let line = CTLineCreateWithAttributedString(NSAttributedString(string: "MixRelay", attributes: attributes))
var commands: [String] = []
func n(_ value: CGFloat) -> String { String(format: "%.3f", Double(value)) }
for run in CTLineGetGlyphRuns(line) as! [CTRun] {
    let count = CTRunGetGlyphCount(run)
    var glyphs = [CGGlyph](repeating: 0, count: count)
    var positions = [CGPoint](repeating: .zero, count: count)
    CTRunGetGlyphs(run, CFRange(location: 0, length: count), &glyphs)
    CTRunGetPositions(run, CFRange(location: 0, length: count), &positions)
    for index in 0..<count {
        guard let outline = CTFontCreatePathForGlyph(font, glyphs[index], nil) else { continue }
        let origin = positions[index]
        func point(_ p: CGPoint) -> String { "\(n(p.x + origin.x)) \(n(-p.y - origin.y))" }
        outline.applyWithBlock { element in
            let e = element.pointee
            switch e.type {
            case .moveToPoint: commands.append("M" + point(e.points[0]))
            case .addLineToPoint: commands.append("L" + point(e.points[0]))
            case .addQuadCurveToPoint: commands.append("Q" + point(e.points[0]) + " " + point(e.points[1]))
            case .addCurveToPoint: commands.append("C" + point(e.points[0]) + " " + point(e.points[1]) + " " + point(e.points[2]))
            case .closeSubpath: commands.append("Z")
            @unknown default: break
            }
        }
    }
}
let data: [String: Any] = [
    "text": "MixRelay", "font": CTFontCopyPostScriptName(font), "tracking": -1.2,
    "width": CTLineGetTypographicBounds(line, nil, nil, nil),
    "capHeight": CTFontGetCapHeight(font), "path": commands.joined()
]
let encoded = try JSONSerialization.data(withJSONObject: data, options: [.prettyPrinted, .sortedKeys])
try encoded.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
