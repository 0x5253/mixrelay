import XCTest
import AppKit
import CoreGraphics

final class InstalledUITests: XCTestCase {
    private var app: InstalledApplication?
    private var editorPid: pid_t?
    private struct MissingControl: Error { let label: String }

    private func press(_ label: String) throws {
        let app = try XCTUnwrap(app)
        let control = try app.wait(label, roles: [kAXButtonRole, kAXPopUpButtonRole, kAXMenuItemRole, kAXCellRole], enabled: true)
        if control.role == kAXMenuItemRole { try control.chooseMenuItem() }
        else { try control.activate() }
    }

    private func createGraph(in directory: URL) throws {
        let expected = try JSONSerialization.jsonObject(with: Data(contentsOf:
            directory.appendingPathComponent("installed-ui-runner-expected.json"))) as? [String: String]
        let actual = ["bundle": Bundle.main.bundleIdentifier ?? "", "path": Bundle.main.bundleURL.path,
                      "pid": String(ProcessInfo.processInfo.processIdentifier), "trusted": String(AXIsProcessTrusted()),
                      "screenCapture": String(CGPreflightScreenCaptureAccess())]
        try JSONSerialization.data(withJSONObject: actual, options: [.prettyPrinted, .sortedKeys])
            .write(to: directory.appendingPathComponent("installed-ui-runner-actual.json"))
        guard actual["bundle"] == expected?["bundle"], actual["path"] == expected?["path"] else {
            throw MissingControl(label: "Granted XCTest runner identity: \(actual)")
        }
        guard AXIsProcessTrusted() else { throw MissingControl(label: "Accessibility access for installed UI runner") }
        guard CGPreflightScreenCaptureAccess() else { throw MissingControl(label: "Screen capture access for installed UI runner") }
        let installed = URL(fileURLWithPath: "/Applications/MixRelay/MixRelay.app").standardizedFileURL
        let editors = NSRunningApplication.runningApplications(withBundleIdentifier: "io.mixrelay.desktop")
            .filter { $0.activationPolicy == .regular && $0.bundleURL?.standardizedFileURL == installed }
        guard editors.count == 1, let editor = editors.first else {
            throw MissingControl(label: "Exactly one installed editor")
        }
        let app = InstalledApplication(pid: editor.processIdentifier)
        self.app = app
        self.editorPid = editor.processIdentifier
        guard editor.activate(options: [.activateAllWindows]) else {
            throw MissingControl(label: "Installed editor PID \(editor.processIdentifier)")
        }
        let activationDeadline = Date().addingTimeInterval(20)
        while !editor.isActive {
            guard Date() < activationDeadline else { throw MissingControl(label: "Installed editor did not become active") }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
        app.editorWindow = try app.waitEditorWindow()
        _ = try app.wait("Add apps, devices, effects and more.", roles: [kAXStaticTextRole])
        try press("Add node")
        try app.wait("Processor", roles: [kAXMenuItemRole]).openSubmenu()
        try press("Compressor")
        _ = try app.wait("Compressor", roles: [kAXGroupRole])
        try press("Add node")
        try app.wait("Output", roles: [kAXMenuItemRole]).openSubmenu()
        try press("Recorder")
        _ = try app.wait("Recorder", roles: [kAXGroupRole])
        try app.wait("Compressor", roles: [kAXGroupRole]).openContextMenu()
        try press("Connect channels…")
        try app.wait("Destination", roles: [kAXButtonRole, kAXPopUpButtonRole], contains: true, enabled: true, timeout: 10).activate()
        try app.waitChoice("Recorder", in: "Destination").activate()
        try press("Connect")
        _ = try app.wait("Compressor CH 1 → Recorder CH 1")
        _ = try app.wait("Recorder: Record", roles: [kAXCheckBoxRole, kAXButtonRole], enabled: true)
        guard app.find("Recording error", roles: [kAXStaticTextRole], contains: true) == nil else {
            throw MissingControl(label: "Recorder must be ready without an error after graph creation")
        }
        try WindowScreenshot.capture(pid: editor.processIdentifier, to: directory.appendingPathComponent("installed-ui.png"))
        try app.debugDescription.write(to: directory.appendingPathComponent("installed-ui-tree.txt"), atomically: true, encoding: .utf8)
    }

    private func prepareAudio(in directory: URL) throws {
        let app = try XCTUnwrap(self.app)
        let pid = try XCTUnwrap(editorPid)
        let window = try XCTUnwrap(app.editorWindow)
        let acknowledgement = AXUIElementSetAttributeValue(window.raw, kAXMinimizedAttribute as CFString, kCFBooleanTrue)
        let deadline = Date().addingTimeInterval(20)
        while window.attribute(kAXMinimizedAttribute) as? Bool != true && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
        guard window.attribute(kAXMinimizedAttribute) as? Bool == true else {
            throw MissingControl(label: "Installed editor window did not minimize: acknowledgement \(acknowledgement.rawValue)")
        }
        try JSONSerialization.data(withJSONObject: ["editorPid": pid, "editorMinimized": true,
            "acknowledgement": acknowledgement.rawValue], options: [.prettyPrinted, .sortedKeys])
            .write(to: directory.appendingPathComponent("installed-ui-audio-phase.json"))
    }

    func testCreateConnectAndAuthorizeNativeAudio() throws {
        continueAfterFailure = true
        let folder = try XCTUnwrap(ProcessInfo.processInfo.environment["MIXRELAY_UI_EVIDENCE"])
        let directory = URL(fileURLWithPath: folder)
        var failure: String?
        do { try createGraph(in: directory) }
        catch {
            if let pid = editorPid {
                try? WindowScreenshot.capture(pid: pid, to: directory.appendingPathComponent("installed-ui-failure.png"))
            }
            let detail = "\(error)\n\(app?.debugDescription ?? "Editor was not selected")"
            XCTFail(detail)
            failure = detail
        }
        do { try prepareAudio(in: directory) }
        catch {
            let detail = "Audio phase preparation: \(error)"
            XCTFail(detail)
            failure = [failure, detail].compactMap { $0 }.joined(separator: "\n")
        }
        if let failure {
            try failure.write(to: directory.appendingPathComponent("installed-ui.error"), atomically: true, encoding: .utf8)
        }
        try Data("ready\n".utf8).write(to: directory.appendingPathComponent("installed-ui.ready"))

        try PermissionObserver(directory: directory).run()
    }
}
