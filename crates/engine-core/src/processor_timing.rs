use mixrelay_control_protocol::ProcessorTiming;

use crate::EngineGraphController;

impl EngineGraphController {
    pub(crate) fn processor_timings(&self) -> Vec<ProcessorTiming> {
        let revision = self.exchange.plan_revisions().active;
        let views = self.execution_views.read();
        let Some(view) = views.get(&revision) else {
            return Vec::new();
        };
        view.origins
            .iter()
            .filter_map(|(runtime_id, origin)| {
                self.processor_controls
                    .processing_time_ns(runtime_id)
                    .map(|round_trip_ns| ProcessorTiming {
                        graph_id: origin.graph_id.clone(),
                        node_id: origin.node_id.as_str().to_owned(),
                        round_trip_ns,
                    })
            })
            .collect()
    }
}
