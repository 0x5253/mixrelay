use super::*;
use crate::{
    endpoints::EndpointSet,
    runtime::{adapter_profile, render_format},
};
use mixrelay_control_protocol::DriverState as Observed;
use mixrelay_engine_core::RuntimeStatusBackend;

#[test]
fn driver_readiness_does_not_require_a_graph_attachment() {
    let status =
        AdapterRuntimeStatus::new(adapter_profile(), Arc::new(mixrelay_time::StdClock::new()));
    let endpoints = EndpointSet::default();
    assert!(endpoints.is_empty());
    observe_driver(&status, Some(DriverState::Ready));
    assert_eq!(status.status(render_format()).driver_state, Observed::Ready);
    observe_driver(&status, Some(DriverState::Unavailable));
    assert_eq!(
        status.status(render_format()).driver_state,
        Observed::Unavailable
    );
    observe_driver(&status, Some(DriverState::NotInstalled));
    assert_eq!(
        status.status(render_format()).driver_state,
        Observed::NotInstalled
    );
    observe_driver(&status, None);
    assert_eq!(
        status.status(render_format()).driver_state,
        Observed::InstalledNotLoaded
    );
}
