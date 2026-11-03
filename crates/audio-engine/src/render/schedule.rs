//! Stage membership is prepared once, while stable indices preserve live controls.
use super::{NodeStep, RenderPhase};

#[derive(Debug)]
pub(super) struct RenderSchedule {
    sources: Box<[usize]>,
    processing: Box<[usize]>,
    sinks: Box<[usize]>,
}

impl RenderSchedule {
    pub(super) fn new(steps: &[NodeStep]) -> Self {
        let stage = |phase| {
            steps
                .iter()
                .enumerate()
                .filter_map(|(index, step)| {
                    (!step.kernels(phase).is_empty()
                    || (matches!(phase, RenderPhase::Processing)
                        && !step.incoming_routes.is_empty())
                    // Terminal consumers such as recorder taps have no sink
                    // kernel, but still require their input-side mute gate.
                    || (matches!(phase, RenderPhase::Sinks)
                        && step.outputs.is_empty() && !step.inputs.is_empty()))
                    .then_some(index)
                })
                .collect()
        };
        Self {
            sources: stage(RenderPhase::Sources),
            processing: stage(RenderPhase::Processing),
            sinks: stage(RenderPhase::Sinks),
        }
    }

    pub(super) fn nodes(&self, phase: RenderPhase) -> &[usize] {
        match phase {
            RenderPhase::Sources => &self.sources,
            RenderPhase::Processing => &self.processing,
            RenderPhase::Sinks => &self.sinks,
        }
    }
}
