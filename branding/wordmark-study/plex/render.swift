import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

// Render the proof PDF; no application input or UI automation.
let url = URL(fileURLWithPath: CommandLine.arguments[1])
let destination = URL(fileURLWithPath: CommandLine.arguments[2])
let scale = CGFloat(Double(CommandLine.arguments[3])!)
guard let doc = CGPDFDocument(url as CFURL), let page = doc.page(at: 1) else { fatalError("Cannot read proof PDF") }
let box = page.getBoxRect(.mediaBox)
let width = Int(box.width * scale), height = Int(box.height * scale)
guard let context = CGContext(data:nil,width:width,height:height,bitsPerComponent:8,bytesPerRow:0,
                             space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else { fatalError("Cannot create bitmap") }
context.setFillColor(CGColor(red:1,green:1,blue:1,alpha:1))
context.fill(CGRect(x:0,y:0,width:width,height:height))
context.scaleBy(x:scale,y:scale); context.drawPDFPage(page)
guard let image = context.makeImage(), let output = CGImageDestinationCreateWithURL(destination as CFURL, UTType.png.identifier as CFString,1,nil) else { fatalError("Cannot write PNG") }
CGImageDestinationAddImage(output,image,nil)
guard CGImageDestinationFinalize(output) else { fatalError("PNG write failed") }
