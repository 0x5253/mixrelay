//! Publication sequence and state generations, independent of ticks and delivery.
use mixrelay_control_protocol::{ControlStateSnapshot, ControlTelemetry};

#[derive(Default)]
pub(super) struct Publication {
    previous: Option<ControlStateSnapshot>,
    sequence: u64,
}

impl Publication {
    pub(super) fn prepare(&mut self, mut packet: ControlTelemetry) -> Option<ControlTelemetry> {
        self.sequence += 1;
        packet.sequence = self.sequence;
        if let Some(state) = &mut packet.state {
            advance(state, self.previous.as_ref());
            self.previous = Some(state.clone());
        }
        packet.state.clone_from(&self.previous);
        packet.state.as_ref()?;
        Some(packet)
    }
}

fn advance(state: &mut ControlStateSnapshot, previous: Option<&ControlStateSnapshot>) {
    state.generation = previous.map_or(1, |old| old.generation);
    if previous.is_some_and(|old| old != state) {
        state.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ControlBackend as _;

    #[test]
    fn deferred_observations_keep_committed_state_and_order_the_next_window() {
        let mut publication = Publication::default();
        let mut packet = super::super::observe(&crate::UnavailableBackend);
        let initial_state = packet.state.take().unwrap();
        assert!(publication.prepare(packet.clone()).is_none());
        packet.state = Some(initial_state);
        let committed = publication.prepare(packet.clone()).unwrap();
        assert_eq!(committed.sequence, 2);
        assert_eq!(committed.state.as_ref().unwrap().generation, 1);
        packet.state = None;
        let deferred = publication.prepare(packet.clone()).unwrap();
        assert_eq!(deferred.sequence, 3);
        assert_eq!(deferred.state, committed.state);
        packet.state = deferred.state;
        packet.state.as_mut().unwrap().graph.active_revision += 1;
        let changed = publication.prepare(packet).unwrap();
        assert_eq!(changed.sequence, 4);
        assert_eq!(changed.state.as_ref().unwrap().generation, 2);
    }

    #[test]
    fn generation_changes_for_each_state_family_but_not_steady_state() {
        let packet = super::super::observe(&crate::UnavailableBackend);
        assert_eq!(crate::UnavailableBackend.telemetry_state(), packet.state);
        let mut state = packet.state.unwrap();
        advance(&mut state, None);
        assert_eq!(state.generation, 1);
        let old = state.clone();
        advance(&mut state, Some(&old));
        assert_eq!(state, old);
        state.status.sample_rate = Some(48_000);
        advance(&mut state, Some(&old));
        assert_eq!(state.generation, 2);
        let old = state.clone();
        state.graph.active_revision += 1;
        advance(&mut state, Some(&old));
        assert_eq!(state.generation, 3);
        let old = state.clone();
        state.controls = Some(mixrelay_control_protocol::EngineControlSnapshot {
            revision: 3,
            active_revision: 3,
            structure_generation: 1,
            engine_power: mixrelay_control_protocol::EnginePowerStatus {
                enabled: true,
                transitioning: false,
                unsaved: false,
                pause_reason: None,
            },
            graphs: vec![],
        });
        advance(&mut state, Some(&old));
        assert_eq!(state.generation, 4);
        let old = state.clone();
        state.editors = vec![mixrelay_control_protocol::ProcessorEditorStatus {
            protocol_version: 1,
            node_id: "effect".into(),
            state: mixrelay_control_protocol::ProcessorEditorState::Closed,
            width: None,
            height: None,
            resizable: false,
            failure: None,
        }];
        advance(&mut state, Some(&old));
        assert_eq!(state.generation, 5);
        let old = state.clone();
        state.controls = None;
        state.editors.clear();
        advance(&mut state, Some(&old));
        assert_eq!(state.generation, 6);
    }
}
