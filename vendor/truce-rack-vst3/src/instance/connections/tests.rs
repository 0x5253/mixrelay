//! Behavioral fixtures for the adjacent native owner.

use super::*;
use std::sync::{Arc, Mutex};
use vst3::{Class, ComWrapper};

struct ConnectionProbe {
    connect_result: i32,
    connect_event: &'static str,
    disconnect_event: &'static str,
    events: Arc<Mutex<Vec<&'static str>>>,
}

impl Class for ConnectionProbe {
    type Interfaces = (IConnectionPoint,);
}

impl IConnectionPointTrait for ConnectionProbe {
    unsafe fn connect(&self, _other: *mut IConnectionPoint) -> i32 {
        self.events
            .lock()
            .expect("connection events")
            .push(self.connect_event);
        self.connect_result
    }

    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> i32 {
        self.events
            .lock()
            .expect("connection events")
            .push(self.disconnect_event);
        kResultOk
    }

    unsafe fn notify(&self, _message: *mut vst3::Steinberg::Vst::IMessage) -> i32 {
        kResultOk
    }
}

fn connection_pair(
    component_result: i32,
    controller_result: i32,
    events: Arc<Mutex<Vec<&'static str>>>,
) -> (ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>) {
    let component = ComWrapper::new(ConnectionProbe {
        connect_result: component_result,
        connect_event: "component.connect",
        disconnect_event: "component.disconnect",
        events: Arc::clone(&events),
    });
    let controller = ComWrapper::new(ConnectionProbe {
        connect_result: controller_result,
        connect_event: "controller.connect",
        disconnect_event: "controller.disconnect",
        events,
    });
    let component_ptr = component
        .to_com_ptr::<IConnectionPoint>()
        .expect("component connection point");
    let controller_ptr = controller
        .to_com_ptr::<IConnectionPoint>()
        .expect("controller connection point");
    (component_ptr, controller_ptr)
}

#[test]
fn connection_guard_rolls_back_failures_and_transfers_complete_pair() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (component_ptr, controller_ptr) = connection_pair(
        vst3::Steinberg::kInvalidArgument,
        kResultOk,
        Arc::clone(&events),
    );
    assert!(connect_pair(component_ptr, controller_ptr).is_err());
    assert_eq!(
        *events.lock().expect("connection events"),
        ["component.connect"]
    );

    events.lock().expect("connection events").clear();
    let (component_ptr, controller_ptr) = connection_pair(
        kResultOk,
        vst3::Steinberg::kInvalidArgument,
        Arc::clone(&events),
    );
    assert!(connect_pair(component_ptr, controller_ptr).is_err());
    assert_eq!(
        *events.lock().expect("connection events"),
        [
            "component.connect",
            "controller.connect",
            "component.disconnect"
        ]
    );

    events.lock().expect("connection events").clear();
    let (component_ptr, controller_ptr) =
        connection_pair(kResultOk, kResultOk, Arc::clone(&events));
    let guard = connect_pair(component_ptr, controller_ptr).expect("complete pair");
    drop(guard);
    assert_eq!(
        *events.lock().expect("connection events"),
        [
            "component.connect",
            "controller.connect",
            "component.disconnect",
            "controller.disconnect"
        ]
    );

    events.lock().expect("connection events").clear();
    let (component_ptr, controller_ptr) =
        connection_pair(kResultOk, kResultOk, Arc::clone(&events));
    let guard = connect_pair(component_ptr, controller_ptr).expect("complete pair");
    let (component, controller) = guard.into_parts();
    assert!(component.is_some() && controller.is_some());
    drop(component);
    drop(controller);
    assert_eq!(
        *events.lock().expect("connection events"),
        ["component.connect", "controller.connect"]
    );
}
