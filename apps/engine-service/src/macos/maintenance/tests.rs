use super::*;
use mixrelay_macos_engine::test_support;
use std::{
    sync::{
        Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[test]
fn stopping_observers_cancels_resource_preparation_before_joining() {
    use mixrelay_engine_core::GraphResourceBackend;

    let status = Arc::new(AdapterRuntimeStatus::new(
        test_support::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    let devices = Arc::new(test_support::devices(
        status,
        48_000,
        32,
        test_support::SimulatedHost::new(),
    ));
    let observers = ControlObservers {
        format_monitor: None,
        control: None,
        devices: Arc::downgrade(&devices),
    };
    observers.stop_observing();

    let error = devices
        .reconcile(
            &mixrelay_audio_graph::GraphDocument::new(1, vec![], vec![]),
            mixrelay_engine_core::PlaybackBindings::Preserve,
        )
        .unwrap_err();
    assert!(error.to_string().contains("cancelled during shutdown"));
    let error = devices.device_inventory(&[]).unwrap_err();
    assert!(error.to_string().contains("cancelled during shutdown"));
    observers.finish().unwrap();
    assert_eq!(Arc::strong_count(&devices), 1);
}

fn observers_with_lifetime_probe() -> (
    ControlObservers,
    Arc<AtomicBool>,
    Weak<EngineGraphController>,
) {
    let controller = Arc::new(crate::EngineGraphController::new_with_configuration(
        Arc::new(mixrelay_render::ProcessorExchange::new(1)),
        crate::EngineGraphControllerConfig {
            clock: Arc::new(mixrelay_time::ManualClock::default()),
            format: mixrelay_audio_graph::RenderFormat {
                sample_rate: 48_000,
                block_frames: 128,
            },
            meters: mixrelay_engine_core::default_peak_meter_bank(),
            initial_bindings: vec![],
            endpoint_backend: Arc::new(
                mixrelay_engine_core::ContractOnlyVirtualEndpointBackend::default(),
            ),
            required_endpoint_node_id: Some("endpoint".into()),
            processor_catalog: mixrelay_processor_host::ProcessorCatalog::default(),
            graph_resources: Arc::new(mixrelay_engine_core::NoopGraphResourceBackend),
            runtime_status: Arc::new(mixrelay_engine_core::FixedRuntimeStatus::new(
                mixrelay_control_protocol::DriverState::Ready,
            )),
        },
    ));
    let control = controller.start_control_maintenance().unwrap();
    let weak = Arc::downgrade(&controller);
    let probe = weak.clone();
    let alive_at_monitor_exit = Arc::new(AtomicBool::new(false));
    let observed = alive_at_monitor_exit.clone();
    let format_monitor = test_support::maintenance_worker("observer-order-test", move |stop| {
        while !stop.load(Ordering::Acquire) {
            thread::park();
        }
        // Only ControlMaintenance owns the controller by this point. Its final
        // checkpoint and owner release must follow the format monitor's join.
        observed.store(probe.upgrade().is_some(), Ordering::Release);
    })
    .unwrap();
    drop(controller);
    (
        ControlObservers {
            format_monitor: Some(format_monitor),
            control: Some(control),
            devices: Weak::new(),
        },
        alive_at_monitor_exit,
        weak,
    )
}

#[test]
fn explicit_shutdown_joins_monitor_before_final_control_owner_release() {
    let (observers, observed, weak) = observers_with_lifetime_probe();

    observers.finish().unwrap();

    assert!(observed.load(Ordering::Acquire));
    assert!(weak.upgrade().is_none());
}

#[test]
fn failed_monitor_reports_its_reason_and_still_finalizes_control() {
    let (mut observers, _, weak) = observers_with_lifetime_probe();
    // Retire the normal monitor before injecting a deterministic failure.
    observers.format_monitor.take().unwrap().finish().unwrap();
    observers.format_monitor = Some(
        test_support::maintenance_worker("failed-format-monitor", |_| {
            panic!("injected format observation failure");
        })
        .unwrap(),
    );

    assert_eq!(
        observers.finish(),
        Err("failed-format-monitor panicked: injected format observation failure".into())
    );
    assert!(weak.upgrade().is_none());
}

#[test]
fn cancelled_server_future_keeps_the_same_observer_shutdown_order() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    let (observers, observed, weak) = observers_with_lifetime_probe();
    let mut serving = Box::pin(async move {
        let _observers = observers;
        std::future::pending::<()>().await;
    });
    let mut context = Context::from_waker(Waker::noop());
    assert_eq!(serving.as_mut().poll(&mut context), Poll::Pending);

    drop(serving);

    assert!(observed.load(Ordering::Acquire));
    assert!(weak.upgrade().is_none());
}

#[test]
fn unwinding_keeps_the_same_observer_shutdown_order() {
    let (observers, observed, weak) = observers_with_lifetime_probe();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _observers = observers;
        panic!("server failed");
    }));

    assert!(result.is_err());
    assert!(observed.load(Ordering::Acquire));
    assert!(weak.upgrade().is_none());
}
