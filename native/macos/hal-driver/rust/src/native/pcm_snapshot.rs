//! Native control snapshots pin each runtime without freezing realtime IO.
use super::Runtime;
use crate::{device::lock, registry::Registry};
use mixrelay_macos_transport::client_pcm::ClientPcmSnapshot;
use std::sync::atomic::Ordering;

type Signature = Vec<(u32, u64)>;

fn signature(registry: &Registry) -> Option<Signature> {
    let mut signature = Vec::new();
    signature.try_reserve_exact(crate::abi::MAX_DEVICES).ok()?;
    for id in registry.devices() {
        let device = registry.pin(id)?;
        let _control = lock(&device.control);
        let _io = device.io_pin()?;
        let runtime = device.runtime.load(Ordering::Acquire).cast::<Runtime>();
        // SAFETY: native Control owns a native_registry; both control and IO pins
        // retain this runtime and exclude client-vector mutation or destruction.
        let runtime = unsafe { runtime.as_ref() }?;
        signature.push((id, runtime.clients.revision()));
    }
    Some(signature)
}

pub(crate) fn client_snapshot(
    registry: &Registry,
    previous: Option<&[(u32, u64)]>,
) -> Option<(Signature, ClientPcmSnapshot)> {
    let current = signature(registry)?;
    if previous == Some(current.as_slice()) {
        return None;
    }
    let mut snapshot = ClientPcmSnapshot::new()?;
    for &(id, revision) in &current {
        let device = registry.pin(id)?;
        let _control = lock(&device.control);
        let _io = device.io_pin()?;
        // SAFETY: the native registry and both pins establish the same ownership as above.
        let runtime = unsafe {
            device
                .runtime
                .load(Ordering::Acquire)
                .cast::<Runtime>()
                .as_ref()
        }?;
        if runtime.clients.revision() != revision
            || !snapshot.push_endpoint(mixrelay_macos_transport::client_pcm::EndpointGeometry {
                id: runtime.io.geometry.endpoint_id,
                render_channels: runtime.io.geometry.render_channels,
                capture_channels: runtime.io.geometry.capture_channels,
            })
            || !runtime.clients.append_snapshot(&mut snapshot)
        {
            return None;
        }
    }
    // A concurrent add/remove invalidates the whole list. A later control tick
    // takes another snapshot; the current authoritative list remains retained.
    (signature(registry)? == current).then_some((current, snapshot))
}
