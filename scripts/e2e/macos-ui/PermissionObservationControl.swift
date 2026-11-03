import Foundation

final class PermissionObservationControl {
    private enum Phase: String, Codable { case paused, observing }
    private struct Command: Codable, Equatable {
        let generation: String
        let phase: Phase
    }
    private struct Acknowledgement: Encodable {
        let generation: String
        let phase: Phase
        let at: String
    }
    private let directory: URL
    private var acknowledged: Command?

    init(directory: URL) { self.directory = directory }

    // Called between scans, so a paused acknowledgement excludes in-flight AX work.
    func shouldObserve() throws -> Bool {
        let path = directory.appendingPathComponent("permission-observation.command.json")
        guard FileManager.default.fileExists(atPath: path.path) else { return true }
        let command = try JSONDecoder().decode(Command.self, from: Data(contentsOf: path))
        if command != acknowledged {
            let state = Acknowledgement(generation: command.generation, phase: command.phase,
                                        at: ISO8601DateFormatter().string(from: Date()))
            try JSONEncoder().encode(state).write(to: directory.appendingPathComponent("permission-observation.ack.json"), options: .atomic)
            acknowledged = command
        }
        return command.phase == .observing
    }
}
