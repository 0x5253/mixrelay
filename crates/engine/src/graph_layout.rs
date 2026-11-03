//! Graph presentation transactions deliberately never prepare audio resources, snapshot
//! hosted processors or publish DSP revisions. A layout edit may address an inactive graph.

use mixrelay_control_protocol::{GraphLayout, GraphLayoutRequest, GraphLayoutResponse};

use super::{EngineGraphController, ErrorCode, ProtocolError};

const MAX_POSITIONS: usize = 4096;
const MAX_COORDINATE: f64 = 10_000_000.0;

fn error(code: ErrorCode, id: &str, reason: &str) -> ProtocolError {
    ProtocolError {
        code,
        parameters: serde_json::json!({ "resource": "graph_layout", "graphId": id, "reason": reason }),
    }
}

fn coordinate(value: f64) -> bool {
    value.is_finite() && value.abs() <= MAX_COORDINATE
}

fn node_identity(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

pub(super) fn validate(layout: &GraphLayout) -> Result<(), &'static str> {
    if layout.positions.len() > MAX_POSITIONS
        || layout.sizes.len() > MAX_POSITIONS
        || layout.node_labels.len() > MAX_POSITIONS
    {
        return Err("layout contains too many node entries");
    }
    if layout.positions.iter().any(|(id, position)| {
        !node_identity(id) || !coordinate(position.x) || !coordinate(position.y)
    }) {
        return Err("layout has an invalid UI node identity or position");
    }
    if layout.sizes.iter().any(|(id, size)| {
        !node_identity(id)
            || !size.width.is_finite()
            || !size.height.is_finite()
            || !(180.0..=4096.0).contains(&size.width)
            || !(26.0..=4096.0).contains(&size.height)
    }) {
        return Err("layout has an invalid UI node identity or size");
    }
    if layout.node_labels.iter().any(|(id, label)| {
        !node_identity(id)
            || label.trim().is_empty()
            || label.chars().count() > 128
            || label.chars().any(char::is_control)
    }) {
        return Err("layout has an invalid UI node identity or display label");
    }
    if let Some(viewport) = layout.viewport
        && (!coordinate(viewport.x)
            || !coordinate(viewport.y)
            || !viewport.zoom.is_finite()
            || !(0.05..=8.0).contains(&viewport.zoom))
    {
        return Err("layout viewport is invalid");
    }
    Ok(())
}

impl EngineGraphController {
    pub(crate) fn read_graph_layout(&self, id: &str) -> Result<GraphLayoutResponse, ProtocolError> {
        self.catalog.inspect(|catalog| {
            let value = catalog
                .ok_or_else(|| {
                    error(
                        ErrorCode::ControlUnavailable,
                        id,
                        "graph catalog is not enabled",
                    )
                })?
                .as_ref()
                .map_err(|reason| error(ErrorCode::ControlUnavailable, id, reason))?;
            let graph = value
                .graphs
                .iter()
                .find(|graph| graph.id == id)
                .ok_or_else(|| error(ErrorCode::GraphInvalid, id, "graph does not exist"))?;
            Ok(GraphLayoutResponse {
                graph_id: id.into(),
                layout_revision: graph.layout_revision,
                layout: graph.layout.clone(),
            })
        })
    }

    pub(crate) fn write_graph_layout(
        &self,
        id: &str,
        request: GraphLayoutRequest,
    ) -> Result<GraphLayoutResponse, ProtocolError> {
        validate(&request.layout).map_err(|reason| error(ErrorCode::GraphInvalid, id, reason))?;
        let _mutation = self.mutation.lock();
        let mut candidate = self.catalog.inspect(|catalog| {
            let value = catalog
                .ok_or_else(|| {
                    error(
                        ErrorCode::ControlUnavailable,
                        id,
                        "graph catalog is not enabled",
                    )
                })?
                .as_ref()
                .map_err(|reason| error(ErrorCode::ControlUnavailable, id, reason))?
                .clone();
            Ok::<_, ProtocolError>(value)
        })?;
        let graph = candidate
            .graphs
            .iter_mut()
            .find(|graph| graph.id == id)
            .ok_or_else(|| error(ErrorCode::GraphInvalid, id, "graph does not exist"))?;
        if request.expected_layout_revision != graph.layout_revision {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: serde_json::json!({
                    "resource": "graph_layout", "graphId": id,
                    "expectedLayoutRevision": request.expected_layout_revision, "layoutRevision": graph.layout_revision,
                }),
            });
        }
        graph.layout_revision = graph
            .layout_revision
            .checked_add(1)
            .ok_or_else(|| error(ErrorCode::GraphInvalid, id, "layout revision overflow"))?;
        graph.layout = request.layout;
        let response = GraphLayoutResponse {
            graph_id: id.into(),
            layout_revision: graph.layout_revision,
            layout: graph.layout.clone(),
        };
        // A failed write has no runtime side effects to reconcile: reject and preserve both
        // the previous disk file and in-memory layout, so retry uses the same revision.
        if let Some(staged) = self
            .stage_catalog(Some(&candidate))
            .map_err(|failure| error(failure.code, id, &failure.parameters.to_string()))?
        {
            staged.commit().map_err(|failure| {
                error(ErrorCode::ControlUnavailable, id, &failure.to_string())
            })?;
        }
        self.commit_catalog_memory(Some(candidate));
        self.catalog.record_warning(None);
        Ok(response)
    }
}
