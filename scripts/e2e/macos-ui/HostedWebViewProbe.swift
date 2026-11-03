import AppKit
import WebKit
import CoreGraphics

@_silgen_name("mr_webkit_set_bool_feature")
func setWebKitFeature(_ preferences: UnsafeRawPointer, _ key: UnsafePointer<CChar>, _ enabled: Bool) -> Int32

final class HostedWebViewProbe: NSObject, NSApplicationDelegate, WKNavigationDelegate {
    let output: URL
    let mode: String
    var window: NSWindow?
    var rendered = false
    init(output: URL, mode: String) { self.output = output; self.mode = mode }

    static func setDOMRendering(_ preferences: WKPreferences, enabled: Bool) -> Bool {
        "UseGPUProcessForDOMRenderingEnabled".withCString {
            setWebKitFeature(Unmanaged.passUnretained(preferences).toOpaque(), $0, enabled) == 0
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let frame = NSRect(x: 80, y: 80, width: 640, height: 480)
        let window = NSWindow(contentRect: frame, styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "MixRelay"
        let configuration = WKWebViewConfiguration()
        if mode != "default" {
            guard Self.setDOMRendering(configuration.preferences, enabled: mode == "remote") else {
                try? Data("Missing WebKit DOM rendering feature".utf8).write(to: output.appendingPathComponent("webkit-probe.error"))
                NSApplication.shared.stop(nil)
                return
            }
        }
        let view = WKWebView(frame: NSRect(origin: .zero, size: frame.size), configuration: configuration)
        view.navigationDelegate = self
        window.contentView = view
        self.window = window
        window.orderFrontRegardless()
        view.loadHTMLString("""
            <!doctype html><html><body style="margin:0;background:#102040;color:white">
            <div style="position:absolute;left:80px;top:80px;width:320px;height:240px;background:#e02040">
            <h1>Native WebKit rendering</h1><button>Fixture control</button></div></body></html>
            """, baseURL: nil)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        let configuration = WKSnapshotConfiguration()
        configuration.afterScreenUpdates = true
        webView.takeSnapshot(with: configuration) { image, error in
            var snapshotRendered = false
            do {
                guard let data = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: data),
                    let png = bitmap.representation(using: .png, properties: [:]) else {
                    throw NSError(domain: "HostedWebViewProbe", code: 1,
                        userInfo: [NSLocalizedDescriptionKey: String(describing: error)])
                }
                snapshotRendered = WindowScreenshot.hasRenderedContent(bitmap)
                try png.write(to: self.output.appendingPathComponent("webkit-snapshot.png"), options: .withoutOverwriting)
                guard CGPreflightScreenCaptureAccess() else {
                    throw NSError(domain: "HostedWebViewProbe", code: 2,
                        userInfo: [NSLocalizedDescriptionKey: "No existing screen capture grant for the rendering fixture"])
                }
                try WindowScreenshot.capture(pid: ProcessInfo.processInfo.processIdentifier,
                    to: self.output.appendingPathComponent("webkit-window.png"))
                self.rendered = true
            } catch {
                try? Data(String(describing: error).utf8).write(to: self.output.appendingPathComponent("webkit-probe.error"))
            }
            try? JSONSerialization.data(withJSONObject: ["rendered": self.rendered,
                "mode": self.mode,
                "snapshotRendered": snapshotRendered,
                "screenCapture": CGPreflightScreenCaptureAccess(),
                "pid": ProcessInfo.processInfo.processIdentifier,
                "os": ProcessInfo.processInfo.operatingSystemVersionString,
                "processors": ProcessInfo.processInfo.processorCount], options: [.prettyPrinted, .sortedKeys])
                .write(to: self.output.appendingPathComponent("webkit-probe.json"), options: .withoutOverwriting)
            NSApplication.shared.stop(nil)
        }
    }
}

@main
enum HostedWebViewProbeMain {
    static func main() {
        let arguments = CommandLine.arguments
        guard arguments.count == 3, ["default", "legacy", "remote"].contains(arguments[1]) else { exit(64) }
        if arguments[1] != "default" {
            UserDefaults.standard.setVolatileDomain([
                "WebKit2UseRemoteLayerTreeDrawingArea": false,
            ], forName: UserDefaults.argumentDomain)
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let delegate = HostedWebViewProbe(output: URL(fileURLWithPath: arguments[2], isDirectory: true), mode: arguments[1])
        app.delegate = delegate
        _ = Timer.scheduledTimer(withTimeInterval: 15, repeats: false) { _ in app.stop(nil) }
        app.run()
        delegate.window?.orderOut(nil)
        exit(delegate.rendered ? 0 : 2)
    }
}
