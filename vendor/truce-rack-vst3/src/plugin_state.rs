//! Versioned VST3 processor and edit-controller state container.

use thiserror::Error;
use vst3::ComPtr;
use vst3::ComWrapper;
use vst3::Steinberg::{IBStream, kNotImplemented, kResultFalse, kResultOk};

use crate::state_stream::{MemoryStream, StreamError};

#[cfg(test)]
mod tests;

const MAGIC: &[u8; 8] = b"MRV3STAT";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 8 + 2 + 1 + 1 + 4 + 4;
const STATE_TRANSPORT_CAPACITY: usize = 64 * 1024 * 1024;
const OUTER_STATE_ENVELOPE_HEADER_LEN: usize = 4 + 2 + 1 + 1 + 4;
const MAX_PLUGIN_STATE_BYTES: usize = STATE_TRANSPORT_CAPACITY - OUTER_STATE_ENVELOPE_HEADER_LEN;
const CONTROLLER_STATE_ABSENT: u8 = 0;
const CONTROLLER_STATE_PRESENT: u8 = 1;

/// Borrowed, fully validated sections of the current VST3 state format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PluginState<'a> {
    component: &'a [u8],
    controller: Option<&'a [u8]>,
}

/// State capture, envelope, or restore failure. Native errors retain their method and status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum PluginStateError {
    /// The current VST3 payload plus its outer rack state header exceeds 64 MiB.
    #[error("VST3 state exceeds the 64 MiB envelope")]
    CapacityExceeded,
    /// A bounded vector reservation failed.
    #[error("VST3 state allocation failed")]
    AllocationFailed,
    /// The container header or declared sections are incomplete.
    #[error("VST3 state container is truncated")]
    Truncated,
    /// The payload is not this VST3 state format; raw component-state fallback is forbidden.
    #[error("VST3 state container magic does not match")]
    BadMagic,
    /// This container version is not supported.
    #[error("unsupported VST3 state container version {0}")]
    UnsupportedVersion(u16),
    /// A controller-presence tag or reserved header byte is invalid.
    #[error("VST3 state container header is invalid")]
    InvalidHeader,
    /// Declared sections do not consume the entire container.
    #[error("VST3 state section lengths do not match the container")]
    LengthMismatch,
    /// A native method returned an unexpected result code.
    #[error("{operation} returned {status}")]
    NativeStatus {
        /// VST3 method that returned the status.
        operation: &'static str,
        /// Native result code.
        status: i32,
    },
    /// `getState` declared controller-state support after writing partial bytes.
    #[error("IEditController::getState returned kNotImplemented after writing state bytes")]
    UnsupportedControllerStateWithData,
    /// A native `IBStream` operation failed even if the plug-in returned success.
    #[error("VST3 state stream failed: {0}")]
    Stream(StreamError),
    /// The host could not expose a valid `IBStream` COM interface.
    #[error("VST3 state stream is missing its IBStream interface")]
    MissingStreamInterface,
}

impl PluginState<'_> {
    /// Capture component then controller state using their SDK-defined call order.
    ///
    /// A controller `getState` result of `kNotImplemented` means its private state is absent,
    /// but only when the call emitted no bytes and did not fail its stream. All other non-success
    /// statuses are errors.
    pub(crate) fn capture(
        component_get_state: impl FnOnce(*mut IBStream) -> i32,
        controller_get_state: impl FnOnce(*mut IBStream) -> i32,
    ) -> Result<Vec<u8>, PluginStateError> {
        Self::capture_with_budget(
            component_get_state,
            controller_get_state,
            MAX_PLUGIN_STATE_BYTES,
        )
    }

    fn capture_with_budget(
        component_get_state: impl FnOnce(*mut IBStream) -> i32,
        controller_get_state: impl FnOnce(*mut IBStream) -> i32,
        total_capacity: usize,
    ) -> Result<Vec<u8>, PluginStateError> {
        if total_capacity > MAX_PLUGIN_STATE_BYTES {
            return Err(PluginStateError::CapacityExceeded);
        }
        let component_capacity = total_capacity
            .checked_sub(HEADER_LEN)
            .ok_or(PluginStateError::CapacityExceeded)?;
        let component = capture_stream(
            component_get_state,
            "IComponent::getState",
            false,
            component_capacity,
        )?
        .ok_or(PluginStateError::InvalidHeader)?;
        checked_total_len(component.len(), 0)?;
        let controller_capacity = component_capacity
            .checked_sub(component.len())
            .ok_or(PluginStateError::CapacityExceeded)?;
        let controller = capture_stream(
            controller_get_state,
            "IEditController::getState",
            true,
            controller_capacity,
        )?;
        encode(&component, controller.as_deref())
    }

    /// Validate all bytes before mutation, then restore component and controller in SDK order.
    ///
    /// `before_mutation` invalidates cached state derived from native latency; it runs only after
    /// the complete envelope has passed validation. A controller `setComponentState` result of
    /// `kResultFalse` is explicitly non-fatal per Steinberg's hosting FAQ. Also accept
    /// `kNotImplemented` at this optional synchronization boundary: released JUCE plug-ins
    /// can return it after synchronizing their shared processor/controller parameters
    /// (JUCE fix 9c192940e02fe264c57f842f92d35cf94e043d4d).
    pub(crate) fn restore(
        bytes: &[u8],
        before_mutation: impl FnOnce(),
        component_set_state: impl FnOnce(*mut IBStream) -> i32,
        controller_set_component_state: impl FnOnce(*mut IBStream) -> i32,
        controller_set_state: impl FnOnce(*mut IBStream) -> i32,
    ) -> Result<(), PluginStateError> {
        let state = Self::decode(bytes)?;
        let component_stream = PreparedRestoreStream::new(state.component)?;
        let controller_component_stream = PreparedRestoreStream::new(state.component)?;
        let controller_state_stream = state
            .controller
            .map(PreparedRestoreStream::new)
            .transpose()?;
        before_mutation();
        component_stream.apply(component_set_state, "IComponent::setState", false)?;
        controller_component_stream.apply(
            controller_set_component_state,
            "IEditController::setComponentState",
            true,
        )?;
        if let Some(controller_stream) = controller_state_stream {
            controller_stream.apply(controller_set_state, "IEditController::setState", false)?;
        }
        Ok(())
    }

    /// Parse the current envelope without allocating or calling a native interface.
    ///
    /// # Errors
    /// Returns an error for old raw component state, unsupported versions, malformed lengths,
    /// unknown controller-state tags, or payloads beyond the existing 64 MiB aggregate limit.
    pub(crate) fn decode(bytes: &'_ [u8]) -> Result<PluginState<'_>, PluginStateError> {
        if bytes.len() > MAX_PLUGIN_STATE_BYTES {
            return Err(PluginStateError::CapacityExceeded);
        }
        if bytes.len() < HEADER_LEN {
            return Err(PluginStateError::Truncated);
        }
        if &bytes[..MAGIC.len()] != MAGIC {
            return Err(PluginStateError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != VERSION {
            return Err(PluginStateError::UnsupportedVersion(version));
        }
        let controller_tag = bytes[10];
        if bytes[11] != 0
            || !matches!(
                controller_tag,
                CONTROLLER_STATE_ABSENT | CONTROLLER_STATE_PRESENT
            )
        {
            return Err(PluginStateError::InvalidHeader);
        }
        let component_len = usize::try_from(read_u32(bytes, 12)?)
            .map_err(|_| PluginStateError::CapacityExceeded)?;
        let controller_len = usize::try_from(read_u32(bytes, 16)?)
            .map_err(|_| PluginStateError::CapacityExceeded)?;
        if controller_tag == CONTROLLER_STATE_ABSENT && controller_len != 0 {
            return Err(PluginStateError::InvalidHeader);
        }
        let expected_len = checked_total_len(component_len, controller_len)?;
        if expected_len != bytes.len() {
            return Err(PluginStateError::LengthMismatch);
        }
        let component_end = HEADER_LEN + component_len;
        let controller =
            (controller_tag == CONTROLLER_STATE_PRESENT).then_some(&bytes[component_end..]);
        Ok(PluginState {
            component: &bytes[HEADER_LEN..component_end],
            controller,
        })
    }
}

fn checked_total_len(
    component_len: usize,
    controller_len: usize,
) -> Result<usize, PluginStateError> {
    let total = HEADER_LEN
        .checked_add(component_len)
        .and_then(|length| length.checked_add(controller_len))
        .ok_or(PluginStateError::CapacityExceeded)?;
    if total > MAX_PLUGIN_STATE_BYTES {
        return Err(PluginStateError::CapacityExceeded);
    }
    Ok(total)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PluginStateError> {
    let field_end = offset.checked_add(4).ok_or(PluginStateError::Truncated)?;
    let field = bytes
        .get(offset..field_end)
        .ok_or(PluginStateError::Truncated)?;
    let bytes: [u8; 4] = field.try_into().map_err(|_| PluginStateError::Truncated)?;
    Ok(u32::from_le_bytes(bytes))
}

fn encode(component: &[u8], controller: Option<&[u8]>) -> Result<Vec<u8>, PluginStateError> {
    let controller_bytes = controller.unwrap_or_default();
    let total = checked_total_len(component.len(), controller_bytes.len())?;
    let component_len =
        u32::try_from(component.len()).map_err(|_| PluginStateError::CapacityExceeded)?;
    let controller_len =
        u32::try_from(controller_bytes.len()).map_err(|_| PluginStateError::CapacityExceeded)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(total)
        .map_err(|_| PluginStateError::AllocationFailed)?;
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.push(if controller.is_some() {
        CONTROLLER_STATE_PRESENT
    } else {
        CONTROLLER_STATE_ABSENT
    });
    bytes.push(0);
    bytes.extend_from_slice(&component_len.to_le_bytes());
    bytes.extend_from_slice(&controller_len.to_le_bytes());
    bytes.extend_from_slice(component);
    bytes.extend_from_slice(controller_bytes);
    Ok(bytes)
}

fn capture_stream(
    call: impl FnOnce(*mut IBStream) -> i32,
    operation: &'static str,
    unsupported_means_absent: bool,
    capacity: usize,
) -> Result<Option<Vec<u8>>, PluginStateError> {
    let memory_stream =
        MemoryStream::with_capacity_limit(capacity).map_err(PluginStateError::Stream)?;
    let stream = ComWrapper::new(memory_stream);
    let stream_ptr = stream
        .to_com_ptr::<IBStream>()
        .ok_or(PluginStateError::MissingStreamInterface)?;
    let status = call(stream_ptr.as_ptr());
    if let Some(error) = stream.stream_error() {
        return Err(PluginStateError::Stream(error));
    }
    let bytes = stream.to_vec().map_err(PluginStateError::Stream)?;
    if let Some(error) = stream.stream_error() {
        return Err(PluginStateError::Stream(error));
    }
    if status == kResultOk {
        return Ok(Some(bytes));
    }
    if unsupported_means_absent && status == kNotImplemented {
        if bytes.is_empty() {
            return Ok(None);
        }
        return Err(PluginStateError::UnsupportedControllerStateWithData);
    }
    Err(PluginStateError::NativeStatus { operation, status })
}

struct PreparedRestoreStream {
    pointer: ComPtr<IBStream>,
    stream: ComWrapper<MemoryStream>,
}

impl PreparedRestoreStream {
    fn new(bytes: &[u8]) -> Result<Self, PluginStateError> {
        let memory_stream = MemoryStream::from_bytes(bytes).map_err(PluginStateError::Stream)?;
        let stream = ComWrapper::new(memory_stream);
        let pointer = stream
            .to_com_ptr::<IBStream>()
            .ok_or(PluginStateError::MissingStreamInterface)?;
        Ok(Self { pointer, stream })
    }

    fn apply(
        &self,
        call: impl FnOnce(*mut IBStream) -> i32,
        operation: &'static str,
        controller_sync: bool,
    ) -> Result<(), PluginStateError> {
        let status = call(self.pointer.as_ptr());
        if let Some(error) = self.stream.stream_error() {
            return Err(PluginStateError::Stream(error));
        }
        if status == kResultOk
            || (controller_sync && (status == kResultFalse || status == kNotImplemented))
        {
            return Ok(());
        }
        Err(PluginStateError::NativeStatus { operation, status })
    }
}
