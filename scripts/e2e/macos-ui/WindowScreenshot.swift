import AppKit
import ScreenCaptureKit

enum WindowScreenshot {
    struct Failure: Error { let message: String }

    static func hasRenderedContent(_ bitmap: NSBitmapImageRep) -> Bool {
        guard bitmap.pixelsWide >= 128, bitmap.pixelsHigh >= 128 else { return false }
        guard let image = bitmap.cgImage, let space = CGColorSpace(name: CGColorSpace.sRGB),
            let pixels = CGContext(data: nil, width: image.width, height: image.height,
                bitsPerComponent: 8, bytesPerRow: image.width * 4, space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue),
            let data = pixels.data else { return false }
        pixels.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        let bytes = data.bindMemory(to: UInt8.self, capacity: image.width * image.height * 4)
        var colors: [UInt32: Int] = [:]
        var samples = 0
        for row in 0..<64 {
            for column in 0..<64 {
                let x = bitmap.pixelsWide / 5 + column * bitmap.pixelsWide * 3 / 320
                let y = bitmap.pixelsHigh / 5 + row * bitmap.pixelsHigh * 3 / 320
                let offset = (y * image.width + x) * 4
                let rgb = (0..<3).reduce(UInt32(0)) { ($0 << 8) | UInt32(bytes[offset + $1]) }
                colors[rgb, default: 0] += 1
                samples += 1
            }
        }
        return samples == 4096 && (colors.values.max() ?? samples) < samples * 99 / 100
    }

    static func capture(pid: pid_t, to output: URL) throws {
        let finished = DispatchSemaphore(value: 0)
        let failure = output.appendingPathExtension("error")
        let metadata = output.deletingPathExtension().appendingPathExtension("window.json")
        func fail(_ message: String) {
            try? Data(message.utf8).write(to: failure, options: .atomic)
            finished.signal()
        }
        SCShareableContent.getExcludingDesktopWindows(true, onScreenWindowsOnly: true) { content, error in
            guard let content = content, error == nil else { fail(String(describing: error)); return }
            let windows = content.windows.filter {
                $0.owningApplication?.processID == pid && $0.title == "MixRelay" && $0.frame.width > 300 && $0.frame.height > 300
            }
            guard windows.count == 1, let window = windows.first else { fail("Expected one installed editor window for PID \(pid)"); return }
            let configuration = SCStreamConfiguration()
            configuration.width = Int(window.frame.width.rounded(.up))
            configuration.height = Int(window.frame.height.rounded(.up))
            configuration.showsCursor = false
            let filter = SCContentFilter(desktopIndependentWindow: window)
            SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration) { image, error in
                guard let image = image, error == nil else { fail(String(describing: error)); return }
                let bitmap = NSBitmapImageRep(cgImage: image)
                let rendered = hasRenderedContent(bitmap)
                do {
                    guard let bytes = bitmap.representation(using: .png, properties: [:]) else { fail("PNG encoding failed"); return }
                    try bytes.write(to: output, options: .withoutOverwriting)
                    try JSONSerialization.data(withJSONObject: ["pid": pid, "windowID": window.windowID,
                        "width": image.width, "height": image.height, "renderedContent": rendered], options: [.prettyPrinted, .sortedKeys])
                        .write(to: metadata, options: .atomic)
                    if !rendered { fail("The installed editor screenshot has a uniform, unrendered body"); return }
                    finished.signal()
                } catch { fail(String(describing: error)) }
            }
        }
        let deadline = Date().addingTimeInterval(20)
        var complete = finished.wait(timeout: .now()) == .success
        while !complete && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            complete = finished.wait(timeout: .now()) == .success
        }
        guard complete else { throw Failure(message: "Installed window capture timed out") }
        if FileManager.default.fileExists(atPath: failure.path) {
            throw Failure(message: (try? String(contentsOf: failure, encoding: .utf8)) ?? "Installed window capture failed")
        }
        guard FileManager.default.fileExists(atPath: output.path) else { throw Failure(message: "Installed window capture produced no PNG") }
    }
}
