//! Engine control routes. Every identifier becomes one percent-encoded path segment, so a
//! slash, query or fragment character in a graph, node or operation ID stays part of it.
use std::fmt::Write as _;

pub const STATUS: &str = "/api/v1/status";
pub const AUDIO_SESSION: &str = "/api/v1/audio/session";
pub const AUDIO_POWER: &str = "/api/v1/audio/power";
pub const AUDIO_POWER_PREVIEW: &str = "/api/v1/audio/power/preview";
pub const VIRTUAL_ENDPOINT_FORMAT: &str = "/api/v1/audio/virtual-endpoint/format";
pub const GRAPH: &str = "/api/v1/graph";
pub const GRAPH_STATUS: &str = "/api/v1/graph/status";
pub const GRAPH_CATALOG: &str = "/api/v1/graphs";
pub const CONTROLS_SNAPSHOT: &str = "/api/v1/controls/snapshot";
pub const METERS: &str = "/api/v1/meters";
/// The engine's server-sent meter windows.
pub const TELEMETRY: &str = "/api/v1/telemetry";
pub const OPERATIONS: &str = "/api/v1/operations";
pub const PROCESSORS: &str = "/api/v1/processors";
pub const PROCESSOR_RETRY: &str = "/api/v1/processors/retry";
pub const PROCESSOR_SCAN: &str = "/api/v1/processors/scan";
pub const WORKSPACE: &str = "/api/v1/workspace";
pub const WORKSPACE_PREVIEW: &str = "/api/v1/workspace/preview";
pub const CAPTURE_MANIFEST: &str = "/api/v1/application-capture/manifest";
pub const CAPTURE_PREVIEW: &str = "/api/v1/application-capture/preview";

/// Percent-encodes everything except RFC 3986 unreserved characters.
#[must_use]
pub fn segment(value: &str) -> String {
    value.bytes().fold(String::new(), |mut encoded, byte| {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
        encoded
    })
}

#[must_use]
pub fn graph_layout(graph_id: &str) -> String {
    format!("{GRAPH_CATALOG}/{}/layout", segment(graph_id))
}

/// The receipt of an accepted operation.
#[must_use]
pub fn operation(operation_id: &str) -> String {
    format!("{OPERATIONS}/{}", segment(operation_id))
}

/// Supported controls and their authoritative states for one graph.
#[must_use]
pub fn node_controls(graph_id: &str) -> String {
    format!("{GRAPH_CATALOG}/{}/nodes/controls", segment(graph_id))
}

#[must_use]
pub fn node_control(graph_id: &str, node_id: &str) -> String {
    format!(
        "{GRAPH_CATALOG}/{}/nodes/{}/control",
        segment(graph_id),
        segment(node_id)
    )
}

/// The dry run of a mutation route, which reports the capture manifest it would need.
#[must_use]
pub fn preview(route: &str) -> String {
    format!("{route}/preview")
}

#[must_use]
pub fn recorder(graph_id: &str, node_id: &str) -> String {
    format!(
        "{GRAPH_CATALOG}/{}/recorders/{}",
        segment(graph_id),
        segment(node_id)
    )
}

#[must_use]
pub fn processor_editor(node_id: &str) -> String {
    format!("{PROCESSORS}/{}/editor", segment(node_id))
}

#[must_use]
pub fn processor_editor_frame(node_id: &str) -> String {
    format!("{}/frame", processor_editor(node_id))
}

#[must_use]
pub fn processor_editor_input(node_id: &str) -> String {
    format!("{}/input", processor_editor(node_id))
}

#[must_use]
pub fn processor_parameter(node_id: &str, parameter_id: u32) -> String {
    format!(
        "{PROCESSORS}/{}/parameters/{parameter_id}",
        segment(node_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_cannot_escape_their_path_segment() {
        assert_eq!(segment("a-b_c.d~e9"), "a-b_c.d~e9");
        assert_eq!(segment("a/b?c#d"), "a%2Fb%3Fc%23d");
        assert_eq!(segment("../ %"), "..%2F%20%25");
        assert_eq!(segment("é"), "%C3%A9");
    }

    #[test]
    fn graph_and_node_identity_stay_separate_in_every_route() {
        assert_eq!(graph_layout("graph-1"), "/api/v1/graphs/graph-1/layout");
        assert_eq!(
            graph_layout("a/b?c#d"),
            "/api/v1/graphs/a%2Fb%3Fc%23d/layout"
        );
        assert_eq!(node_controls("main"), "/api/v1/graphs/main/nodes/controls");
        assert_eq!(
            node_control("graph/2", "processor/?#"),
            "/api/v1/graphs/graph%2F2/nodes/processor%2F%3F%23/control"
        );
        assert_eq!(
            preview(&node_control("g", "n")),
            "/api/v1/graphs/g/nodes/n/control/preview"
        );
        assert_eq!(
            recorder("a/b", "c?d"),
            "/api/v1/graphs/a%2Fb/recorders/c%3Fd"
        );
        assert_eq!(operation("a/b?c"), "/api/v1/operations/a%2Fb%3Fc");
    }

    #[test]
    fn processor_routes_address_one_node() {
        assert_eq!(processor_editor("n/1"), "/api/v1/processors/n%2F1/editor");
        assert_eq!(
            processor_editor_frame("n"),
            "/api/v1/processors/n/editor/frame"
        );
        assert_eq!(
            processor_editor_input("n"),
            "/api/v1/processors/n/editor/input"
        );
        assert_eq!(
            processor_parameter("n?", 7),
            "/api/v1/processors/n%3F/parameters/7"
        );
    }
}
