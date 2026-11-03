//! ID-directed device negotiation. The controller owns suspension, verified
//! native writes, DSP publication and rollback; the device backend owns lookup.
use super::{
    EngineGraphController, NodeConfig, SessionChangeOrigin, SessionPauseOwner, session_error,
};
use mixrelay_audio_io::VirtualEndpointId;
use mixrelay_control_protocol::{
    ErrorCode, GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION, ProtocolError,
    VirtualEndpointFormatRequest,
};
use mixrelay_time::Deadline;
use std::time::Duration;

// CoreAudio accepts nominal-rate changes before PerformDeviceConfigurationChange
// commits them. This budget runs only on the control server's blocking worker;
// graph activation has its own deadline after the HTTP response is delivered.
const RATE_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const RATE_ACK_INTERVAL: Duration = Duration::from_millis(10);

impl EngineGraphController {
    pub(super) fn change_endpoint_format(
        &self,
        request: &VirtualEndpointFormatRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        self.validate_endpoint_format_request(request)?;
        let previous = self
            .endpoint_backend
            .sample_rate(&request.endpoint_id)
            .map_err(|error| session_error(&error.to_string()))?;
        if previous == request.sample_rate {
            return self.publish_endpoint_observation(SessionPauseOwner::SessionChange);
        }
        if let Err(error) = self.runtime_status.suspend() {
            self.runtime_status.resume();
            return Err(session_error(&error.to_string()));
        }
        // A DAW can change CoreAudio while the callback acknowledges pause.
        // Never overwrite that newer observation with this stale request.
        if self.endpoint_backend.sample_rate(&request.endpoint_id).ok() != Some(previous) {
            self.runtime_status.resume();
            return Err(session_error("device rate changed before negotiation"));
        }
        let result = self
            .write_endpoint_rate(&request.endpoint_id, previous, request.sample_rate)
            .and_then(|()| self.publish_endpoint_observation(SessionPauseOwner::EndpointChange));
        if let Err(error) = &result
            && let Err(rollback) = self.rollback_endpoint_rate(request, previous)
        {
            self.runtime_status.fail_reconfiguration();
            return Err(session_error(&format!(
                "device format update failed: {error:?}; rollback failed: {rollback:?}"
            )));
        }
        self.runtime_status.resume();
        result
    }

    fn validate_endpoint_format_request(
        &self,
        request: &VirtualEndpointFormatRequest,
    ) -> Result<(), ProtocolError> {
        self.require_active_revision(request.expected_revision)?;
        if !self.graph.read().nodes.iter().any(|node| {
            matches!(&node.config, NodeConfig::VirtualEndpoint(endpoint)
                if endpoint.id == request.endpoint_id)
        }) {
            return Err(session_error(
                "virtual device is not referenced by this graph",
            ));
        }
        let settings = self
            .endpoint_settings()
            .ok_or_else(|| session_error("virtual device settings unavailable"))?;
        let device = settings
            .devices
            .iter()
            .find(|device| device.spec.id == request.endpoint_id)
            .ok_or_else(|| session_error("virtual device settings unavailable"))?;
        if !device.can_set_sample_rate
            || !device.supported_sample_rates.contains(&request.sample_rate)
        {
            return Err(ProtocolError {
                code: ErrorCode::AudioSessionUnsupported,
                parameters: serde_json::json!({"resource":"virtual_endpoint_format",
                    "endpointId":request.endpoint_id,"sampleRate":request.sample_rate}),
            });
        }
        Ok(())
    }

    pub(crate) fn write_endpoint_rate(
        &self,
        id: &VirtualEndpointId,
        previous: u32,
        rate: u32,
    ) -> Result<(), ProtocolError> {
        let deadline = Deadline::after(self.clock.now(), RATE_ACK_TIMEOUT);
        self.endpoint_backend
            .set_sample_rate(id, rate)
            .map_err(|error| session_error(&error.to_string()))?;
        loop {
            match self.endpoint_backend.sample_rate(id) {
                Ok(observed) if observed == rate => return Ok(()),
                Ok(observed) if observed != previous => {
                    return Err(session_error(
                        "device changed externally during negotiation",
                    ));
                }
                // The old rate and temporary absence are both expected while
                // native configuration is pending; neither confirms a failure.
                Ok(_) | Err(_) => {}
            }
            let remaining = deadline.remaining(self.clock.now());
            if remaining.is_zero() {
                return Err(session_error(
                    "device sample-rate acknowledgement timed out",
                ));
            }
            self.clock.sleep_until(Deadline::after(
                self.clock.now(),
                remaining.min(RATE_ACK_INTERVAL),
            ));
        }
    }

    fn rollback_endpoint_rate(
        &self,
        request: &VirtualEndpointFormatRequest,
        previous: u32,
    ) -> Result<(), ProtocolError> {
        match self.endpoint_backend.sample_rate(&request.endpoint_id) {
            Ok(rate) if rate == previous || rate == request.sample_rate => {
                // Even an unchanged readback may hide the accepted request that
                // just timed out. Supersede that generation before resuming DSP;
                // skipping the setter would let it commit after this rollback.
                self.write_endpoint_rate(&request.endpoint_id, rate, previous)
            }
            _ => Err(session_error(
                "device changed externally; rollback would overwrite it",
            )),
        }
    }

    fn publish_endpoint_observation(
        &self,
        pause_owner: SessionPauseOwner,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let current = self.context.read().format;
        // Only the actual callback clock owner can change the processing format.
        // Editing a secondary VIO must not move unrelated hardware to its clock.
        if let Some(observed) = self.runtime_status.external_format(current)
            && observed != current
        {
            return self.change_session_locked(
                mixrelay_control_protocol::SessionReconfigureRequest {
                    expected_revision: self.exchange.revisions().submitted,
                    format: mixrelay_audio_io::session::SessionFormat {
                        sample_rate: observed.sample_rate,
                        block_frames: observed.block_frames,
                    },
                },
                SessionChangeOrigin::Observed,
                pause_owner,
            );
        }
        self.reconcile_endpoint_clock_locked().map(|response| {
            response.unwrap_or(GraphApplyResponse {
                protocol_version: PROTOCOL_VERSION,
                revision: self.exchange.revisions().active,
                state: GraphApplyState::Active,
                persistence_error: None,
            })
        })
    }
}
