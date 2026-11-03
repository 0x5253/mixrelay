import XCTest

final class InstalledApplicationTests: XCTestCase {
    func testDestinationChoiceIsScopedToTheListForStaticTextAndCellWebKitProviders() {
        let tree = ["app": ["node", "wrong-list", "destination"], "wrong-list": ["other"], "destination": ["option"]]
        let labels = ["node": ["Recorder"], "other": ["Recorder"], "option": ["Recorder"], "wrong-list": ["Other"], "destination": ["Destination"]]
        for optionRole in [kAXStaticTextRole, kAXCellRole, kAXRowRole] {
            let roles = ["node": kAXStaticTextRole, "other": optionRole, "option": optionRole, "wrong-list": kAXListRole, "destination": kAXListRole]
            XCTAssertEqual(InstalledApplication.choice(root: "app", label: "Recorder", listLabel: "Destination", children: { tree[$0] ?? [] }, role: { roles[$0] ?? "" }, labels: { labels[$0] ?? [] }), "option")
            XCTAssertNil(InstalledApplication.choice(root: "app", label: "Compressor", listLabel: "Destination", children: { tree[$0] ?? [] }, role: { roles[$0] ?? "" }, labels: { labels[$0] ?? [] }))
        }
    }
    func testSearchStaysInsideSelectedEditorTree() {
        let trees = ["editor": ["MixRelay", "Add node"], "runtime": ["Quit"]]
        XCTAssertEqual(InstalledApplication.first(root: "editor", children: { trees[$0] ?? [] }, matches: { $0 == "Add node" }), "Add node")
        XCTAssertNil(InstalledApplication.first(root: "runtime", children: { trees[$0] ?? [] }, matches: { $0 == "Add node" }))
    }

    func testSearchStopsAtBrokenProviderCycle() {
        var visits = 0
        XCTAssertNil(InstalledApplication.first(root: 1, children: { [$0] }, matches: { _ in visits += 1; return false }))
        XCTAssertEqual(visits, 10_000)
    }

    func testPermissionObserverUsesGuiOwnersAndOnlyApprovesQualificationPrompts() {
        XCTAssertFalse(PermissionPrompt.isOwner("com.apple.tccd"))
        XCTAssertTrue(PermissionPrompt.isOwner("com.apple.UserNotificationCenter"))
        XCTAssertLessThan(PermissionPrompt.priority("com.apple.UserNotificationCenter"), PermissionPrompt.priority("com.apple.controlcenter"))
        XCTAssertTrue(PermissionPrompt.belongsToQualification(["Microphone", "mixrelay-service wants access"]))
        XCTAssertTrue(PermissionPrompt.belongsToQualification(["MRRoundTripProbe wants access"]))
        XCTAssertTrue(PermissionPrompt.belongsToQualification(["MRSourceControl wants access"]))
        XCTAssertFalse(PermissionPrompt.belongsToQualification(["Another app wants access", "OK"]))
        XCTAssertTrue(PermissionPrompt.isApproval(["OK"]))
        XCTAssertTrue(PermissionPrompt.isApproval(["Allow"]))
        XCTAssertFalse(PermissionPrompt.isApproval(["Don't Allow"]))
    }

    func testPermissionIdentitySurvivesNativeElementChangesAndDistinguishesSubjects() {
        let microphone = ["_NS:91", "MixRelay microphone", "Allow", "action-button-1"]
        let refreshed = ["action-button-1", "_NS:74", "Allow", "MixRelay microphone"]
        XCTAssertEqual(PermissionPrompt.identity(owner: 10, labels: microphone), PermissionPrompt.identity(owner: 10, labels: refreshed))
        XCTAssertNotEqual(PermissionPrompt.identity(owner: 10, labels: microphone), PermissionPrompt.identity(owner: 10, labels: ["MixRelay system audio"]))
        XCTAssertNotEqual(PermissionPrompt.identity(owner: 10, labels: microphone), PermissionPrompt.identity(owner: 11, labels: microphone))
        XCTAssertNotEqual(PermissionPrompt.identity(owner: 10, labels: microphone), PermissionPrompt.identity(owner: 10, labels: ["MixRelay Source Control microphone", "Allow"]))
        XCTAssertEqual(PermissionPrompt.identity(owner: 10, labels: microphone), PermissionPrompt.identity(owner: 10, labels: ["MIXRELAY MICROPHONE", "MixRelay captures applications", "Allow"]))
        XCTAssertNil(PermissionPrompt.identity(owner: 10, labels: ["MixRelay", "Allow"]))
        XCTAssertNil(PermissionPrompt.identity(owner: 10, labels: ["Other app microphone", "Allow"]))
    }
}
