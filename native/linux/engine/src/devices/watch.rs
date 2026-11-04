//! Which device bindings the recovery worker must reopen, decided from shared
//! health and selection cells without touching native streams.
use super::{DeviceRequest, DeviceSelection, DeviceStreams};
use mixrelay_audio_graph::RenderFormat;
use mixrelay_audio_io::{DeviceDirection, DeviceSelector};
use mixrelay_engine_core::resource_requirements::DeviceRequirements;
use mixrelay_linux_audio_io::PipeWireTarget;
use mixrelay_linux_pipewire::StreamOwners;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Read-only health shared with the recovery worker, never a shared stream/ring.
#[derive(Clone)]
pub(crate) struct DeviceWatch {
    pub(super) owners: StreamOwners,
    pub(crate) generation: u64,
    pub(super) spec: DeviceRequirements,
    pub(super) inputs: BTreeMap<String, Arc<AtomicBool>>,
    pub(super) outputs: BTreeMap<String, Arc<AtomicBool>>,
    pub(super) input_selections: BTreeMap<String, Arc<DeviceSelection>>,
    pub(super) output_selections: BTreeMap<String, Arc<DeviceSelection>>,
    pub(crate) format: RenderFormat,
    #[cfg(test)]
    pub(super) automatic_outputs: bool,
}

impl DeviceWatch {
    pub(super) fn of(streams: &DeviceStreams, spec: DeviceRequirements) -> Self {
        let health = |owners: &BTreeMap<String, super::ObservedStream>| {
            owners
                .iter()
                .map(|(id, value)| (id.clone(), value.connected.clone()))
                .collect()
        };
        let selections = |owners: &BTreeMap<String, super::ObservedStream>| {
            owners
                .iter()
                .map(|(id, value)| (id.clone(), value.selected.clone()))
                .collect()
        };
        Self {
            owners: streams.owners.clone(),
            #[cfg(test)]
            automatic_outputs: streams.automatic_outputs,
            format: streams.format.unwrap_or_else(crate::runtime::render_format),
            generation: streams.generation,
            spec,
            inputs: health(&streams.inputs),
            outputs: health(&streams.outputs),
            input_selections: selections(&streams.inputs),
            output_selections: selections(&streams.outputs),
        }
    }

    /// Enabled bindings whose acquired stream disconnected. A binding without a
    /// selection cell has not been gated off and is retried too.
    pub(crate) fn disconnected(&self) -> DeviceRequirements {
        DeviceRequirements {
            inputs: disconnected(&self.spec.inputs, &self.inputs, &self.input_selections),
            outputs: disconnected(&self.spec.outputs, &self.outputs, &self.output_selections),
        }
    }

    /// Whether recovery has anything to watch: a lost stream, or an enabled binding
    /// that follows a system default which may move.
    pub(crate) fn needs_observation(&self) -> bool {
        !self.disconnected().is_empty()
            || follows_enabled_default(&self.spec.inputs, &self.input_selections)
                .next()
                .is_some()
            || follows_enabled_default(&self.spec.outputs, &self.output_selections)
                .next()
                .is_some()
    }

    /// Disconnected bindings plus enabled default bindings whose system default
    /// now resolves to a different device serial.
    pub(super) fn recovery_requests(&self, targets: &[PipeWireTarget]) -> DeviceRequirements {
        let mut spec = self.disconnected();
        self.add_moved_defaults(
            &mut spec.inputs,
            &self.spec.inputs,
            &self.input_selections,
            targets,
        );
        self.add_moved_defaults(
            &mut spec.outputs,
            &self.spec.outputs,
            &self.output_selections,
            targets,
        );
        spec
    }

    fn add_moved_defaults<R: DeviceRequest>(
        &self,
        pending: &mut Vec<(String, R)>,
        requests: &[(String, R)],
        selections: &BTreeMap<String, Arc<DeviceSelection>>,
        targets: &[PipeWireTarget],
    ) {
        for (id, request) in follows_enabled_default(requests, selections) {
            let resolved = request
                .resolve(targets)
                .ok()
                .and_then(|target| target.serial.parse::<u64>().ok());
            if self.changed_default(id, request.selector(), R::DIRECTION, resolved)
                && !pending.iter().any(|(queued, _)| queued == id)
            {
                pending.push((id.clone(), request.clone()));
            }
        }
    }

    /// An enabled binding that follows the system default of `direction` is
    /// selected on a different device than the one `resolved` now names.
    pub(super) fn changed_default(
        &self,
        id: &str,
        selector: &DeviceSelector,
        direction: DeviceDirection,
        resolved: Option<u64>,
    ) -> bool {
        let selections = match (selector, direction) {
            (DeviceSelector::DefaultInput, DeviceDirection::Input) => &self.input_selections,
            (DeviceSelector::DefaultOutput, DeviceDirection::Output) => &self.output_selections,
            _ => return false,
        };
        resolved.is_some_and(|serial| {
            selections.get(id).is_some_and(|current| {
                current.enabled() && current.serial.load(Ordering::Acquire) != serial
            })
        })
    }
}

fn disconnected<R: DeviceRequest>(
    requests: &[(String, R)],
    health: &BTreeMap<String, Arc<AtomicBool>>,
    selections: &BTreeMap<String, Arc<DeviceSelection>>,
) -> Vec<(String, R)> {
    requests
        .iter()
        .filter(|(id, _)| {
            selections.get(id).is_none_or(|value| value.enabled())
                && health
                    .get(id)
                    .is_some_and(|value| !value.load(Ordering::Acquire))
        })
        .cloned()
        .collect()
}

fn follows_enabled_default<'a, R: DeviceRequest>(
    requests: &'a [(String, R)],
    selections: &'a BTreeMap<String, Arc<DeviceSelection>>,
) -> impl Iterator<Item = (&'a String, &'a R)> {
    requests.iter().filter_map(|(id, request)| {
        selections.get(id).filter(|value| value.enabled())?;
        request.follows_default().then_some((id, request))
    })
}
