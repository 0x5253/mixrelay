use super::super::native::{DescriptionChange, NativeStream, StreamProperties};
use super::{EndpointSet, EndpointSpecs, HostAudioBackendError, RenderFormat};
use std::collections::BTreeMap;

/// Control-side capabilities never keep an unreferenced native stream alive.
#[derive(Clone, Default)]
pub(crate) struct EndpointControls(BTreeMap<String, EndpointControlEntry>);

#[derive(Clone)]
struct EndpointControlEntry {
    streams: Vec<StreamProperties>,
}

impl EndpointSet {
    /// Builds the next control inventory off RT, preserving acquisition-specific
    /// weak capabilities for every retained owner. Missing slots reject staging.
    /// # Errors
    /// Rejects a retained slot without a previously registered capability.
    pub(crate) fn controls(&self, previous: &EndpointControls) -> Result<EndpointControls, String> {
        self.nodes
            .iter()
            .map(|(id, slot)| {
                let controls = if let Some(owner) = &slot.owner {
                    let streams = owner
                        .input
                        .iter()
                        .chain(owner.output.iter())
                        .map(NativeStream::properties)
                        .collect();
                    EndpointControlEntry { streams }
                } else {
                    previous
                        .0
                        .get(id)
                        .ok_or("retained PipeWire endpoint has no control capability")?
                        .clone()
                };
                Ok((id.clone(), controls))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()
            .map(EndpointControls)
    }
}

/// All storage, native acquisitions and description strings are prepared off RT.
pub(crate) struct EndpointEdit {
    endpoints: Box<EndpointSet>,
    controls: EndpointControls,
    descriptions: Vec<DescriptionChange>,
}

fn failure(message: String) -> HostAudioBackendError {
    HostAudioBackendError::BackendFailure { message }
}

impl EndpointEdit {
    /// Prepares native owners and acquisition-pinned metadata changes off RT.
    /// # Errors
    /// Rejects invalid specs/names, retired controls, or failed native creation.
    pub(crate) fn prepare(
        owners: &super::super::native::StreamOwners,
        desired: &EndpointSpecs,
        previous: &EndpointSpecs,
        controls: &EndpointControls,
        format: RenderFormat,
    ) -> Result<Self, HostAudioBackendError> {
        let endpoints = Box::new(
            EndpointSet::prepare(owners, desired, previous, format, format).map_err(failure)?,
        );
        let controls = endpoints.controls(controls).map_err(failure)?;
        let mut descriptions = Vec::new();
        for (id, slot) in &endpoints.nodes {
            if slot.owner.is_some() {
                continue;
            }
            let before = previous
                .get(id)
                .ok_or_else(|| failure("retained endpoint spec missing".into()))?;
            if before.display_name == slot.spec.display_name {
                continue;
            }
            let streams = controls
                .0
                .get(id)
                .ok_or_else(|| failure("prepared endpoint controls missing".into()))?;
            for control in &streams.streams {
                descriptions.push(
                    control
                        .description_change(&before.display_name, &slot.spec.display_name)
                        .map_err(failure)?,
                );
            }
        }
        Ok(Self {
            endpoints,
            controls,
            descriptions,
        })
    }

    /// Metadata writes happen only after other resources are prepared. Failed
    /// writes/publication roll back the already changed native descriptions;
    /// rollback errors are returned explicitly, never hidden by a success result.
    /// # Errors
    /// Returns the original preparation/publication error and any rollback failures.
    pub(crate) fn publish(
        self,
        publish: impl FnOnce(Box<EndpointSet>) -> Result<(), HostAudioBackendError>,
    ) -> Result<EndpointControls, HostAudioBackendError> {
        for (index, change) in self.descriptions.iter().enumerate() {
            if let Err(error) = change.apply() {
                return Err(rollback(&self.descriptions[..=index], failure(error)));
            }
        }
        if let Err(error) = publish(self.endpoints) {
            return Err(rollback(&self.descriptions, error));
        }
        Ok(self.controls)
    }
}

fn rollback(
    changes: &[DescriptionChange],
    original: HostAudioBackendError,
) -> HostAudioBackendError {
    let mut errors = Vec::new();
    for change in changes.iter().rev() {
        if let Err(error) = change.rollback() {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        original
    } else {
        failure(format!(
            "{original}; description rollback failed: {}",
            errors.join("; ")
        ))
    }
}
