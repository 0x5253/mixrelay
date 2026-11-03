import AppKit
import ApplicationServices

final class PermissionObserver {
    private let directory: URL
    private let control: PermissionObservationControl
    private struct Deadline: Error {}

    init(directory: URL) {
        self.directory = directory
        self.control = PermissionObservationControl(directory: directory)
    }

    func run() throws {
        // The same disposable session handles the real first-use privacy dialogs.
        // No privacy database or application binary is changed by this observer.
        let deadline = Date().addingTimeInterval(300)
        var grants: [[String: Any]] = []
        var submitted: [String: (index: Int, owner: pid_t, dialog: AXUIElement)] = [:]
        while !FileManager.default.fileExists(atPath: directory.appendingPathComponent("installed-ui.finish").path) {
            if Date() >= deadline { throw Deadline() }
            guard try control.shouldObserve() else {
                RunLoop.current.run(until: Date().addingTimeInterval(1))
                continue
            }
            var observed: [pid_t: [InstalledElement]] = [:]
            var changed = false
            // tccd is a daemon, not a launchable GUI application. Asking XCTest for
            // its state raises an Objective-C exception and kills this observer.
            for owner in NSWorkspace.shared.runningApplications.sorted(by: {
                PermissionPrompt.priority($0.bundleIdentifier ?? "") < PermissionPrompt.priority($1.bundleIdentifier ?? "")
            }) {
                guard let bundle = owner.bundleIdentifier, PermissionPrompt.isOwner(bundle) else { continue }
                let gui = InstalledApplication(pid: owner.processIdentifier, messagingTimeout: 0.2)
                guard let children = gui.root.availableChildren else { continue }
                observed[owner.processIdentifier] = children
                let dialogs = children.filter { [kAXWindowRole, kAXSheetRole].contains($0.role) }
                for dialog in dialogs {
                    let approval = PermissionPrompt.approval(in: dialog)
                    guard PermissionPrompt.belongsToQualification(approval.labels) else { continue }
                    guard let identity = PermissionPrompt.identity(owner: owner.processIdentifier, labels: approval.labels) else { continue }
                    guard submitted[identity] == nil, let button = approval.buttons.first else { continue }
                    let action = button.labels.joined(separator: "/")
                    try button.pressNative()
                    submitted[identity] = (grants.count, owner.processIdentifier, dialog.raw)
                    grants.append(["owner": bundle, "action": action, "subject": approval.labels, "at": ISO8601DateFormatter().string(from: Date())])
                    changed = true
                }
            }
            for submission in submitted.values {
                guard let windows = observed[submission.owner], grants[submission.index]["dismissedAt"] == nil,
                      !windows.contains(where: { CFEqual($0.raw, submission.dialog) }) else { continue }
                grants[submission.index]["dismissedAt"] = ISO8601DateFormatter().string(from: Date())
                changed = true
            }
            if changed {
                try JSONSerialization.data(withJSONObject: grants, options: [.prettyPrinted, .sortedKeys])
                    .write(to: directory.appendingPathComponent("audio-permission-actions.json"), options: .atomic)
            }
            RunLoop.current.run(until: Date().addingTimeInterval(1))
        }
    }
}
