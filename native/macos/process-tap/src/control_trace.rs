//! Native control procedure receipts, consumed by the desktop log subscriber.

use crate::TapError;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Procedure {
    CreateDescription,
    CreateTap,
    ReadTapFormat,
    AddFormatListener,
    AddInventoryListener,
    AddServiceListener,
    CreateAggregate,
    AggregateReady,
    RegisterIo,
    StartIo,
    StopIo,
    UnregisterIo,
    RemoveFormatListener,
    RemoveInventoryListener,
    RemoveServiceListener,
    DestroyAggregate,
    DestroyTap,
}

impl Procedure {
    const fn name(self) -> &'static str {
        match self {
            Self::CreateDescription => "mr_tap_description_create",
            Self::CreateTap => "AudioHardwareCreateProcessTap",
            Self::ReadTapFormat => "AudioObjectGetPropertyData:tap_format",
            Self::AddFormatListener => "AudioObjectAddPropertyListener:tap_format",
            Self::AddInventoryListener => "AudioObjectAddPropertyListener:process_inventory",
            Self::AddServiceListener => "AudioObjectAddPropertyListener:service_restart",
            Self::CreateAggregate => "AudioHardwareCreateAggregateDevice",
            Self::AggregateReady => "aggregate_input_readiness",
            Self::RegisterIo => "AudioDeviceCreateIOProcID",
            Self::StartIo => "AudioDeviceStart",
            Self::StopIo => "AudioDeviceStop",
            Self::UnregisterIo => "AudioDeviceDestroyIOProcID",
            Self::RemoveFormatListener => "AudioObjectRemovePropertyListener:tap_format",
            Self::RemoveInventoryListener => "AudioObjectRemovePropertyListener:process_inventory",
            Self::RemoveServiceListener => "AudioObjectRemovePropertyListener:service_restart",
            Self::DestroyAggregate => "AudioHardwareDestroyAggregateDevice",
            Self::DestroyTap => "AudioHardwareDestroyProcessTap",
        }
    }
}

pub(crate) fn run<T>(
    procedure: Procedure,
    action: impl FnOnce() -> Result<T, TapError>,
) -> Result<T, TapError> {
    // Subscriber-owned span IDs correlate concurrent control calls without a
    // process-global session counter or any selected application's identity.
    let span =
        tracing::info_span!(target: "mixrelay_process_tap::control", "native_capture_control");
    let call_id = span.id().map_or(0, |id| id.into_u64());
    let _entered = span.enter();
    tracing::info!(target: "mixrelay_process_tap::control", platform = "macos",
        procedure = procedure.name(), call_id, event = "entered", "native capture procedure");
    let result = action();
    let error = result.as_ref().err().copied().unwrap_or_default();
    tracing::info!(target: "mixrelay_process_tap::control", platform = "macos",
        procedure = procedure.name(), call_id, event = "returned", code = error.code,
        os_status = error.os_status, "native capture procedure");
    result
}

#[cfg(test)]
mod tests;
