import Foundation
import CoreText
import CoreGraphics

// Source outlines only; this study does not replace the selected production wordmark.
let output = CommandLine.arguments[1]
let requested = CommandLine.arguments[2]
let text = CommandLine.arguments[3]
let tracking = Double(CommandLine.arguments[4])!
let adjustments = CommandLine.arguments.count > 5
  ? try JSONSerialization.jsonObject(with: Data(CommandLine.arguments[5].utf8)) as! [String: Double]
  : [:]
let font = CTFontCreateWithName(requested as CFString, 100, nil)
let actual = CTFontCopyPostScriptName(font) as String
guard actual == requested else { fatalError("Font substitution: \(requested) → \(actual)") }
let attributed = NSMutableAttributedString(string: text, attributes: [
  NSAttributedString.Key(kCTFontAttributeName as String): font
])
let characters = Array(text)
for index in 0..<(characters.count - 1) {
  let pair = String(characters[index...index+1])
  attributed.addAttribute(NSAttributedString.Key(kCTKernAttributeName as String),
    value: tracking + (adjustments[pair] ?? 0), range: NSRange(location: index, length: 1))
}
let line = CTLineCreateWithAttributedString(attributed)
var commands: [String] = []
var union = CGRect.null
var glyphMetrics: [[String: Any]] = []
func n(_ value: CGFloat) -> String { String(format: "%.4f", Double(value)) }
for run in CTLineGetGlyphRuns(line) as! [CTRun] {
  let count = CTRunGetGlyphCount(run)
  var glyphs = [CGGlyph](repeating: 0, count: count)
  var positions = [CGPoint](repeating: .zero, count: count)
  var indices = [CFIndex](repeating: 0, count: count)
  CTRunGetGlyphs(run, CFRange(location: 0, length: count), &glyphs)
  CTRunGetPositions(run, CFRange(location: 0, length: count), &positions)
  CTRunGetStringIndices(run, CFRange(location: 0, length: count), &indices)
  for index in 0..<count {
    guard let outline = CTFontCreatePathForGlyph(font, glyphs[index], nil) else { continue }
    let origin = positions[index]
    let bounds = outline.boundingBoxOfPath.offsetBy(dx: origin.x, dy: origin.y)
    union = union.union(bounds)
    glyphMetrics.append(["character": String(characters[indices[index]]),
      "bounds": [bounds.minX, -bounds.maxY, bounds.maxX, -bounds.minY]])
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
let data: [String: Any] = ["text": text, "font": actual, "tracking": tracking,
  "pairAdjustments": adjustments, "glyphs": glyphMetrics,
  "width": CTLineGetTypographicBounds(line, nil, nil, nil),
  "capHeight": CTFontGetCapHeight(font),
  "alignmentHeight": union.maxY,
  "bounds": [union.minX, -union.maxY, union.maxX, -union.minY],
  "path": commands.joined()]
try JSONSerialization.data(withJSONObject: data, options: [.prettyPrinted, .sortedKeys])
  .write(to: URL(fileURLWithPath: output))
