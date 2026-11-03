use super::*;
use std::sync::{Arc, Mutex};
use vst3::{Class, ComWrapper};

struct Probe {
    name: &'static str,
    status: i32,
    events: Arc<Mutex<Vec<&'static str>>>,
}
impl Class for Probe {
    type Interfaces = (IPluginBase,);
}
impl IPluginBaseTrait for Probe {
    unsafe fn initialize(&self, _: *mut FUnknown) -> i32 {
        self.status
    }
    unsafe fn terminate(&self) -> i32 {
        self.events.lock().unwrap().push(self.name);
        kResultOk
    }
}

fn initialized(name: &'static str, events: &Arc<Mutex<Vec<&'static str>>>) -> InitializedPlugin {
    let plugin = ComWrapper::new(Probe {
        name,
        status: kResultOk,
        events: Arc::clone(events),
    });
    let pointer = plugin.to_com_ptr::<IPluginBase>().unwrap();
    InitializedPlugin::initialize(&pointer, std::ptr::null_mut()).unwrap()
}

#[test]
fn ownership_transfer_terminates_controller_before_component_exactly_once() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let component = initialized("component", &events);
    let controller = initialized("controller", &events);
    let owner = PluginLifetimes::new(component, Some(controller));
    assert!(events.lock().unwrap().is_empty());
    drop(owner);
    assert_eq!(*events.lock().unwrap(), ["controller", "component"]);

    events.lock().unwrap().clear();
    drop(PluginLifetimes::new(initialized("combined", &events), None));
    assert_eq!(*events.lock().unwrap(), ["combined"]);
}

#[test]
fn rejected_initialization_is_not_terminated_and_partial_success_is_reclaimed() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let component = initialized("component", &events);
    let plugin = ComWrapper::new(Probe {
        name: "rejected",
        status: vst3::Steinberg::kInvalidArgument,
        events: Arc::clone(&events),
    });
    let pointer = plugin.to_com_ptr::<IPluginBase>().unwrap();
    assert!(InitializedPlugin::initialize(&pointer, std::ptr::null_mut()).is_err());
    assert!(events.lock().unwrap().is_empty());
    drop(component);
    assert_eq!(*events.lock().unwrap(), ["component"]);
}
