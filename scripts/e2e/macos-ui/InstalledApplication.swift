import AppKit
import ApplicationServices

// AX roots retain the selected process identity even when two roles share a bundle ID.
struct InstalledElement {
    let raw: AXUIElement
    private static let rightArrowKey: CGKeyCode = 124
    private static let returnKey: CGKeyCode = 36
    private static let f10Key: CGKeyCode = 109

    func attribute(_ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(raw, name as CFString, &value) == .success else { return nil }
        return value
    }

    var children: [InstalledElement] {
        availableChildren ?? []
    }

    var availableChildren: [InstalledElement]? {
        (attribute(kAXChildrenAttribute) as? [AXUIElement])?.map { InstalledElement(raw: $0) }
    }

    var role: String { attribute(kAXRoleAttribute) as? String ?? "" }
    var labels: [String] {
        [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute, kAXIdentifierAttribute]
            .compactMap { attribute($0) as? String }
    }

    func matches(_ label: String, roles: [String] = [], contains: Bool = false) -> Bool {
        (roles.isEmpty || roles.contains(role)) && labels.contains {
            contains ? $0.contains(label) : $0 == label
        }
    }

    func click(right: Bool = false) throws {
        guard let position = attribute(kAXPositionAttribute), CFGetTypeID(position) == AXValueGetTypeID(),
              let size = attribute(kAXSizeAttribute), CFGetTypeID(size) == AXValueGetTypeID() else {
            throw InstalledApplication.Failure.control("Element has no clickable bounds: \(labels)")
        }
        var point = CGPoint.zero
        var extent = CGSize.zero
        guard AXValueGetValue(position as! AXValue, .cgPoint, &point),
              AXValueGetValue(size as! AXValue, .cgSize, &extent), extent.width > 0, extent.height > 0 else {
            throw InstalledApplication.Failure.control("Element has empty bounds: \(labels)")
        }
        point.x += extent.width / 2
        point.y += extent.height / 2
        guard let move = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved,
                                 mouseCursorPosition: point, mouseButton: .left) else {
            throw InstalledApplication.Failure.control("Could not move pointer")
        }
        move.post(tap: .cghidEventTap)
        let button: CGMouseButton = right ? .right : .left
        for type: CGEventType in right ? [.rightMouseDown, .rightMouseUp] : [.leftMouseDown, .leftMouseUp] {
            guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: button) else {
                throw InstalledApplication.Failure.control("Could not create mouse event")
            }
            event.post(tap: .cghidEventTap)
        }
    }

    func pressNative() throws {
        let acknowledgement = AXUIElementPerformAction(raw, kAXPressAction as CFString)
        guard acknowledgement == .success else {
            throw InstalledApplication.Failure.control("Native approval failed: \(labels), acknowledgement \(acknowledgement.rawValue)")
        }
    }

    private func focus() throws {
        let acknowledgement = AXUIElementSetAttributeValue(raw, kAXFocusedAttribute as CFString, kCFBooleanTrue)
        let deadline = Date().addingTimeInterval(30)
        while attribute(kAXFocusedAttribute) as? Bool != true {
            guard Date() < deadline else { throw InstalledApplication.Failure.control("Control never received focus: \(labels), acknowledgement \(acknowledgement.rawValue)") }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        }
    }

    private func sendKey(_ key: CGKeyCode, flags: CGEventFlags = []) throws {
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: nil, virtualKey: key, keyDown: down) else {
                throw InstalledApplication.Failure.control("Could not create keyboard event")
            }
            event.flags = flags
            event.post(tap: .cghidEventTap)
        }
    }

    func openSubmenu() throws {
        try focus()
        try sendKey(Self.rightArrowKey)
    }

    func chooseMenuItem() throws {
        guard role == kAXMenuItemRole else {
            throw InstalledApplication.Failure.control("Expected a menu item: \(labels)")
        }
        try activate()
    }

    func activate() throws {
        try focus()
        try sendKey(Self.returnKey)
    }

    func openContextMenu() throws {
        try focus()
        try sendKey(Self.f10Key, flags: .maskShift)
    }
}

final class InstalledApplication {
    enum Failure: Error { case control(String) }
    let root: InstalledElement
    let pid: pid_t
    var editorWindow: InstalledElement?

    init(pid: pid_t, messagingTimeout: Float = 2) {
        self.pid = pid
        root = InstalledElement(raw: AXUIElementCreateApplication(pid))
        AXUIElementSetMessagingTimeout(root.raw, messagingTimeout)
    }

    // The traversal is shared with synthetic trees in the unit tests. A broken AX
    // provider cannot keep a search alive by returning cycles or unbounded children.
    static func first<Node>(root: Node, children: (Node) -> [Node], matches: (Node) -> Bool) -> Node? {
        var pending = [root]
        var count = 0
        while let node = pending.popLast(), count < 10_000 {
            count += 1
            if matches(node) { return node }
            pending.append(contentsOf: children(node).reversed())
        }
        return nil
    }

    func find(_ label: String, roles: [String] = [], contains: Bool = false) -> InstalledElement? {
        Self.first(root: editorWindow ?? root, children: { $0.children }) { $0.matches(label, roles: roles, contains: contains) }
    }

    static func choice<Node>(root: Node, label: String, listLabel: String, children: (Node) -> [Node], role: (Node) -> String, labels: (Node) -> [String]) -> Node? {
        guard let list = first(root: root, children: children, matches: {
            role($0) == kAXListRole && labels($0).contains(listLabel)
        }) else { return nil }
        // Older WebKit exposes an ARIA option as AXStaticText, rather than AXCell.
        // Restrict that fallback to the named list, never the graph's node titles.
        return first(root: list, children: children, matches: {
            [kAXStaticTextRole, kAXCellRole, kAXRowRole].contains(role($0)) && labels($0).contains(label)
        })
    }

    func waitChoice(_ label: String, in listLabel: String, timeout: TimeInterval = 30) throws -> InstalledElement {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if let option = Self.choice(root: editorWindow ?? root, label: label, listLabel: listLabel, children: { $0.children }, role: { $0.role }, labels: { $0.labels }) { return option }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        throw Failure.control("Missing choice \(label) in list \(listLabel) in editor PID \(pid)")
    }

    func wait(_ label: String, roles: [String] = [], contains: Bool = false, enabled: Bool = false, timeout: TimeInterval = 30) throws -> InstalledElement {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if let element = find(label, roles: roles, contains: contains),
               !enabled || element.attribute(kAXEnabledAttribute) as? Bool == true { return element }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        throw Failure.control("Missing control \(label) in editor PID \(pid)")
    }

    func waitEditorWindow(timeout: TimeInterval = 20) throws -> InstalledElement {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            if let window = root.children.first(where: { $0.matches("MixRelay", roles: [kAXWindowRole]) }) { return window }
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        } while Date() < deadline
        throw Failure.control("Missing editor window in PID \(pid)")
    }

    var debugDescription: String {
        var lines = ["Installed editor PID \(pid)"]
        guard let window = editorWindow else { return lines.joined(separator: "\n") + "\nEditor window unavailable" }
        _ = Self.first(root: window, children: { $0.children }) { element in
            lines.append("\(element.role): \(element.labels)")
            return false
        }
        return lines.joined(separator: "\n")
    }
}
