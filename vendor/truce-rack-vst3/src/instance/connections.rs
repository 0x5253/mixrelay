//! Disconnect partial and complete controller/component peer connections.

use truce_rack_core::error::{Error, Result};
use vst3::{
    ComPtr,
    Steinberg::{
        Vst::{IComponent, IConnectionPoint, IConnectionPointTrait, IEditController},
        kResultOk,
    },
};

pub(super) fn connect_peers(
    component: &ComPtr<IComponent>,
    controller: &ComPtr<IEditController>,
    separate: bool,
) -> Result<PeerConnections> {
    if !separate {
        return Ok(PeerConnections::empty());
    }
    let component_peer = component.as_com_ref().cast::<IConnectionPoint>();
    let controller_peer = controller.as_com_ref().cast::<IConnectionPoint>();
    if let (Some(component), Some(controller)) = (&component_peer, &controller_peer) {
        return connect_pair(component.clone(), controller.clone());
    }
    Ok(PeerConnections::empty())
}

fn connect_pair(
    component: ComPtr<IConnectionPoint>,
    controller: ComPtr<IConnectionPoint>,
) -> Result<PeerConnections> {
    // SAFETY: both retained interfaces remain alive through each synchronous call; this pair is
    // connected and owned on the serialized plugin lifecycle thread.
    if unsafe { component.connect(controller.as_ptr()) } != kResultOk {
        return Err(Error::Other(
            "VST3 component IConnectionPoint::connect failed".into(),
        ));
    }
    // SAFETY: same retained pair and lifecycle thread. Roll back the first successful connection
    // if the reverse connection is rejected; never transfer a half-connected pair to the owner.
    if unsafe { controller.connect(component.as_ptr()) } != kResultOk {
        unsafe { component.disconnect(controller.as_ptr()) };
        return Err(Error::Other(
            "VST3 controller IConnectionPoint::connect failed".into(),
        ));
    }
    Ok(PeerConnections {
        component: Some(component),
        controller: Some(controller),
        connected: true,
    })
}

pub(super) struct PeerConnections {
    component: Option<ComPtr<IConnectionPoint>>,
    controller: Option<ComPtr<IConnectionPoint>>,
    connected: bool,
}

impl PeerConnections {
    fn empty() -> Self {
        Self {
            component: None,
            controller: None,
            connected: false,
        }
    }

    pub(super) fn into_parts(
        mut self,
    ) -> (
        Option<ComPtr<IConnectionPoint>>,
        Option<ComPtr<IConnectionPoint>>,
    ) {
        self.connected = false;
        (self.component.take(), self.controller.take())
    }
}

impl Drop for PeerConnections {
    fn drop(&mut self) {
        if self.connected
            && let (Some(component), Some(controller)) = (&self.component, &self.controller)
        {
            // SAFETY: this guard retains both connected interfaces until it disconnects them.
            unsafe { component.disconnect(controller.as_ptr()) };
            // SAFETY: same retained pair; disconnect the reverse peer before releasing either.
            unsafe { controller.disconnect(component.as_ptr()) };
        }
    }
}

#[cfg(test)]
mod tests;
