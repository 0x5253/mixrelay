//! Native resource identity extracted from a composed graph on the control thread.
//! This owns descriptions only, never streams. Gain stays in DSP, so changing it
//! cannot invalidate a native stream cache. Adapters retain format and OS policy.
use mixrelay_audio_graph::{
    ApplicationCaptureRequest, AudioDeviceInputConfig, AudioDeviceOutputConfig, GraphDocument,
    NodeConfig, VirtualEndpointSpec,
};

/// Owned physical-device descriptions that adapters compose with native state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceRequirements {
    /// Canonical graph binding IDs paired with unity-gain native input descriptions.
    pub inputs: Vec<(String, AudioDeviceInputConfig)>,
    /// Canonical graph binding IDs paired with native output descriptions.
    pub outputs: Vec<(String, AudioDeviceOutputConfig)>,
}

impl DeviceRequirements {
    /// Whether the graph requires no physical input or output streams.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.outputs.is_empty()
    }

    /// Extracts device identity and channel widths without retaining graph-local gain.
    /// The document must already have passed graph validation and composition.
    #[must_use]
    pub fn from_document(document: &GraphDocument) -> Self {
        let mut devices = Self::default();
        for node in &document.nodes {
            devices.collect(node.id.as_str(), &node.config);
        }
        devices
    }

    fn collect(&mut self, id: &str, config: &NodeConfig) {
        match config {
            NodeConfig::AudioDeviceInput(config) => {
                let mut config = config.clone();
                config.gain_db = 0.0;
                self.inputs.push((id.to_owned(), config));
            }
            NodeConfig::AudioDeviceOutput(config) => {
                self.outputs.push((id.to_owned(), config.clone()));
            }
            _ => {}
        }
    }
}

/// Platform-neutral descriptions, not a claim that every adapter supports them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResourceRequirements {
    /// Canonical pre-mute capture IDs and selection, with gain left to DSP.
    /// Graph-local playback intents travel separately with the prepared control bank.
    pub captures: Vec<(String, ApplicationCaptureRequest)>,
    /// Physical device descriptions; no native handles or stream owners.
    pub devices: DeviceRequirements,
    /// Driver-owned device references requiring native graph clients, never device creation.
    /// Adapters read capture and send only graph contributions to render.
    pub endpoints: Vec<(String, VirtualEndpointSpec)>,
}

impl ResourceRequirements {
    /// Extracts native requirements from a validated, composed execution document.
    /// Preserves binding IDs and order; does not resolve selectors or open resources.
    #[must_use]
    pub fn from_document(document: &GraphDocument) -> Self {
        let mut requirements = Self::default();
        for node in &document.nodes {
            requirements.devices.collect(node.id.as_str(), &node.config);
            match &node.config {
                NodeConfig::ApplicationCapture(request) => {
                    let mut request = request.clone();
                    request.gain_db = 0.0;
                    requirements
                        .captures
                        .push((node.id.as_str().to_owned(), request));
                }
                NodeConfig::VirtualEndpointTransport(spec) => {
                    requirements
                        .endpoints
                        .push((node.id.as_str().to_owned(), spec.clone()));
                }
                _ => {}
            }
        }
        requirements
    }
}

#[cfg(test)]
mod tests;
