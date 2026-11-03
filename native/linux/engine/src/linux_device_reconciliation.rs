use super::{DeviceRequirements, DeviceStreams, DeviceWatch, ObservedStream};
use mixrelay_audio_graph::RenderFormat;
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

/// Index prior descriptions once on the control thread. An unchanged request
/// reserves a vacant slot carrying the original acquisition lease, not a handle.
fn partition<C: Clone + PartialEq>(
    requested: &[(String, C)],
    previous: &[(String, C)],
    leases: &BTreeMap<String, Arc<AtomicBool>>,
    slots: &mut BTreeMap<String, ObservedStream>,
) -> Vec<(String, C)> {
    let previous = previous
        .iter()
        .map(|(id, config)| (id.as_str(), config))
        .collect::<BTreeMap<_, _>>();
    let mut required = Vec::new();
    for (id, config) in requested {
        if previous
            .get(id.as_str())
            .is_some_and(|before| *before == config)
            && let Some(lease) = leases.get(id)
        {
            slots.insert(
                id.clone(),
                ObservedStream {
                    stream: None,
                    connected: lease.clone(),
                    selected: Arc::new(super::DeviceSelection::new(id, 0)),
                    serial: 0,
                    replaces: None,
                },
            );
        } else {
            required.push((id.clone(), config.clone()));
        }
    }
    required
}

#[cfg(test)]
#[path = "linux_device_reconciliation_tests.rs"]
mod tests;

impl DeviceStreams {
    /// Builds an edit without opening streams or touching active PCM cursors.
    /// Format changes deliberately start a new recovery epoch and retain nothing.
    pub(super) fn stage(
        spec: &DeviceRequirements,
        generation: u64,
        format: RenderFormat,
        previous: Option<&DeviceWatch>,
    ) -> (Self, DeviceRequirements) {
        let empty = Self::default().watch(DeviceRequirements::default());
        let previous = previous.filter(|watch| watch.format == format);
        let mut staged = Self {
            generation: previous.map_or(generation, |watch| watch.generation),
            format: Some(format),
            ..Self::default()
        };
        let previous = previous.unwrap_or(&empty);
        let inputs = partition(
            &spec.inputs,
            &previous.spec.inputs,
            &previous.inputs,
            &mut staged.inputs,
        );
        let outputs = partition(
            &spec.outputs,
            &previous.spec.outputs,
            &previous.outputs,
            &mut staged.outputs,
        );
        staged.inherit_leases(previous);
        (staged, DeviceRequirements { inputs, outputs })
    }

    /// Whole-transaction preflight before any native resource is transferred.
    /// Arc identity fences removal/re-add even when the graph binding ID is reused.
    pub(crate) fn can_retain_from(&self, current: &Self) -> bool {
        [
            (&self.inputs, &current.inputs),
            (&self.outputs, &current.outputs),
        ]
        .iter()
        .all(|(next, active)| {
            next.iter().all(|(id, slot)| {
                slot.stream.is_some()
                    || (self.generation == current.generation
                        && active.get(id).is_some_and(|owner| {
                            owner.stream.is_some() && Arc::ptr_eq(&slot.connected, &owner.connected)
                        }))
            })
        })
    }

    /// Exclusive RT transfer after preflight. No allocation, reclamation, locks
    /// or native reconnect; newly opened input cursors alone discard stale PCM.
    pub(crate) fn retain_from(&mut self, current: &mut Self) {
        self.discard_inputs();
        for (next, active) in [
            (&mut self.inputs, &mut current.inputs),
            (&mut self.outputs, &mut current.outputs),
        ] {
            for (id, slot) in next {
                if slot.stream.is_none()
                    && let Some(owner) = active.get_mut(id)
                {
                    std::mem::swap(slot, owner);
                }
            }
        }
    }

    /// Recovery carries the disconnected acquisition's existing health/lease,
    /// so RT can reject results for subsequently removed or reselected devices.
    pub(super) fn inherit_leases(&mut self, watch: &DeviceWatch) {
        for (streams, leases) in [
            (&mut self.inputs, &watch.inputs),
            (&mut self.outputs, &watch.outputs),
        ] {
            for (id, stream) in streams {
                if let Some(lease) = leases.get(id) {
                    stream.connected = lease.clone();
                }
            }
        }
        for (streams, selections) in [
            (&mut self.inputs, &watch.input_selections),
            (&mut self.outputs, &watch.output_selections),
        ] {
            for (id, stream) in streams {
                if let Some(selected) = selections.get(id) {
                    stream.selected = selected.clone();
                }
            }
        }
    }
}
