use super::*;
use std::{
    cell::{Cell, RefCell},
    ffi::CStr,
    ptr,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::Duration,
};
use vst3::{
    ComRef,
    Steinberg::{
        IBStream, IPlugView, IPluginBaseTrait,
        Vst::{
            IAttributeList, IAttributeList_iid, IAttributeListTrait, IComponentHandlerTrait,
            IEditController, IEditControllerTrait, IMessage, IMessage_iid, IMessageTrait,
            ParameterInfo, String128,
        },
        kResultFalse,
    },
};

struct ProbeBase {
    contexts: Arc<Mutex<Vec<usize>>>,
    terminations: Arc<AtomicUsize>,
}

impl Class for ProbeBase {
    type Interfaces = (IPluginBase,);
}

impl IPluginBaseTrait for ProbeBase {
    unsafe fn initialize(&self, context: *mut FUnknown) -> i32 {
        if context.is_null() {
            return kResultOk;
        }
        let Some(context_ref) = (unsafe { ComRef::<FUnknown>::from_raw(context) }) else {
            return kInvalidArgument;
        };
        if context_ref.cast::<IHostApplication>().is_none() {
            return kInvalidArgument;
        }
        self.contexts
            .lock()
            .expect("probe lock")
            .push(context.addr());
        kResultOk
    }

    unsafe fn terminate(&self) -> i32 {
        self.terminations.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
}

#[test]
fn host_application_reports_name_and_truthful_unsupported_service() {
    let application = ComWrapper::new(HostApplication);
    let pointer = application
        .to_com_ptr::<IHostApplication>()
        .expect("host app");
    let mut name: String128 = [u16::MAX; 128];
    assert_eq!(unsafe { pointer.getName(&raw mut name) }, kResultOk);
    assert_eq!(String::from_utf16_lossy(&name[..8]), "MixRelay");
    assert_eq!(name[8], 0);

    let mut cid = [0; 16];
    let mut iid = [0; 16];
    let mut object = std::ptr::dangling_mut::<std::ffi::c_void>();
    assert_eq!(
        unsafe { pointer.createInstance(&raw mut cid, &raw mut iid, &raw mut object) },
        kNoInterface
    );
    assert!(object.is_null());

    let mut class_id = IMessage_iid;
    let mut mismatched_interface_id = IAttributeList_iid;
    assert_eq!(
        unsafe {
            pointer.createInstance(
                &raw mut class_id,
                &raw mut mismatched_interface_id,
                &raw mut object,
            )
        },
        kNoInterface
    );
    assert!(object.is_null());
    assert_eq!(
        unsafe { pointer.getName(std::ptr::null_mut()) },
        kInvalidArgument
    );
    assert_eq!(
        unsafe { pointer.createInstance(&raw mut cid, &raw mut iid, std::ptr::null_mut()) },
        kInvalidArgument
    );
}

#[test]
fn host_application_clears_output_for_null_class_or_interface_id() {
    let application = ComWrapper::new(HostApplication);
    let host = application
        .to_com_ptr::<IHostApplication>()
        .expect("host app");
    let mut interface_id = IMessage_iid;
    let mut class_id = IMessage_iid;
    let mut object = std::ptr::dangling_mut::<std::ffi::c_void>();
    assert_eq!(
        unsafe {
            host.createInstance(std::ptr::null_mut(), &raw mut interface_id, &raw mut object)
        },
        kInvalidArgument
    );
    assert!(object.is_null());

    object = std::ptr::dangling_mut::<std::ffi::c_void>();
    assert_eq!(
        unsafe { host.createInstance(&raw mut class_id, std::ptr::null_mut(), &raw mut object,) },
        kInvalidArgument
    );
    assert!(object.is_null());
}

fn create_message_and_attributes() -> (ComPtr<IMessage>, ComPtr<IAttributeList>) {
    let application = ComWrapper::new(HostApplication);
    let host = application
        .to_com_ptr::<IHostApplication>()
        .expect("host app");
    let mut class_id = IMessage_iid;
    let mut interface_id = IMessage_iid;
    let mut object = ptr::null_mut();
    assert_eq!(
        unsafe { host.createInstance(&raw mut class_id, &raw mut interface_id, &raw mut object) },
        kResultOk
    );
    let message = unsafe { ComPtr::<IMessage>::from_raw(object.cast()) }.expect("message");
    let raw_attributes = unsafe { message.getAttributes() };
    // SAFETY: the message owns this interface while the retained COM reference is acquired.
    let attributes = unsafe { ComRef::<IAttributeList>::from_raw(raw_attributes) }
        .expect("message attributes")
        .to_com_ptr();
    (message, attributes)
}

#[test]
fn host_message_sets_id_and_typed_scalars() {
    let (message, attributes) = create_message_and_attributes();
    unsafe { message.setMessageID(c"parameter-set".as_ptr()) };
    let message_id = unsafe { message.getMessageID() };
    assert_eq!(
        unsafe { CStr::from_ptr(message_id) }.to_bytes(),
        b"parameter-set"
    );

    assert_eq!(
        unsafe { attributes.setInt(c"rindex".as_ptr(), 4) },
        kResultOk
    );
    let mut index = -1;
    assert_eq!(
        unsafe { attributes.getInt(c"rindex".as_ptr(), &raw mut index) },
        kResultOk
    );
    assert_eq!(index, 4);
    assert_eq!(
        unsafe { attributes.setFloat(c"value".as_ptr(), 0.625) },
        kResultOk
    );
    let mut value = -1.0;
    assert_eq!(
        unsafe { attributes.getFloat(c"value".as_ptr(), &raw mut value) },
        kResultOk
    );
    assert_eq!(value.to_bits(), 0.625_f64.to_bits());
    assert_eq!(
        unsafe { attributes.getInt(c"missing".as_ptr(), &raw mut index) },
        kResultFalse
    );
    assert_eq!(
        unsafe { attributes.getInt(c"value".as_ptr(), &raw mut index) },
        kResultFalse
    );
}

#[test]
fn retained_message_attributes_survive_message_drop() {
    let (message, attributes) = create_message_and_attributes();
    drop(message);
    assert_eq!(
        unsafe { attributes.setInt(c"after-drop".as_ptr(), 7) },
        kResultOk
    );
    let mut value = 0;
    assert_eq!(
        unsafe { attributes.getInt(c"after-drop".as_ptr(), &raw mut value) },
        kResultOk
    );
    assert_eq!(value, 7);
}

#[test]
fn message_attributes_own_string_and_binary_inputs() {
    let (_message, attributes) = create_message_and_attributes();
    let expected_text = (*b"gain\0").map(u16::from);
    let mut source_text = expected_text;
    assert_eq!(
        unsafe { attributes.setString(c"text".as_ptr(), source_text.as_ptr()) },
        kResultOk
    );
    source_text[0] = u16::from(b'x');
    assert_ne!(source_text, expected_text);
    let mut copied_text = [u16::MAX; 5];
    assert_eq!(
        unsafe { attributes.getString(c"text".as_ptr(), copied_text.as_mut_ptr(), 10) },
        kResultOk
    );
    assert_eq!(copied_text, expected_text);

    let mut source_binary = [3_u8, 1, 4];
    assert_eq!(
        unsafe { attributes.setBinary(c"binary".as_ptr(), source_binary.as_ptr().cast(), 3) },
        kResultOk
    );
    source_binary[0] = 9;
    assert_eq!(source_binary, [9, 1, 4]);
    let mut binary = ptr::null();
    let mut binary_size = 0;
    assert_eq!(
        unsafe { attributes.getBinary(c"binary".as_ptr(), &raw mut binary, &raw mut binary_size) },
        kResultOk
    );
    assert_eq!(binary_size, 3);
    // SAFETY: the list owns this copied binary value and it is not replaced during comparison.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(binary.cast::<u8>(), 3) },
        &[3, 1, 4]
    );
}

#[test]
fn message_string_copy_obeys_byte_capacity_and_partial_terminator() {
    let (_message, attributes) = create_message_and_attributes();
    let text = (*b"gain\0").map(u16::from);
    assert_eq!(
        unsafe { attributes.setString(c"text".as_ptr(), text.as_ptr()) },
        kResultOk
    );
    let expected_bytes = text
        .into_iter()
        .flat_map(u16::to_ne_bytes)
        .collect::<Vec<_>>();

    let mut zero_capacity = [0x7777_u16];
    assert_eq!(
        unsafe { attributes.getString(c"text".as_ptr(), zero_capacity.as_mut_ptr(), 0) },
        kResultOk
    );
    assert_eq!(zero_capacity, [0x7777]);

    let mut no_terminator = [0x5a5a_u16; 5];
    assert_eq!(
        unsafe { attributes.getString(c"text".as_ptr(), no_terminator.as_mut_ptr(), 8) },
        kResultOk
    );
    assert_eq!(&no_terminator[..4], &(*b"gain").map(u16::from));
    assert_eq!(no_terminator[4], 0x5a5a);

    let mut odd_capacity = [0x5a5a_u16; 2];
    assert_eq!(
        unsafe { attributes.getString(c"text".as_ptr(), odd_capacity.as_mut_ptr(), 3) },
        kResultOk
    );
    // SAFETY: inspect bytes within the full two-u16 backing buffer.
    let odd_bytes = unsafe { std::slice::from_raw_parts(odd_capacity.as_ptr().cast::<u8>(), 4) };
    let expected_prefix = [u16::from(b'g'), u16::from(b'a')]
        .into_iter()
        .flat_map(u16::to_ne_bytes)
        .take(3)
        .collect::<Vec<_>>();
    assert_eq!(&odd_bytes[..3], expected_prefix.as_slice());
    assert_eq!(odd_bytes[3], 0x5a);

    let mut partial_nul = [0x5a5a_u16; 5];
    assert_eq!(
        unsafe { attributes.getString(c"text".as_ptr(), partial_nul.as_mut_ptr(), 9) },
        kResultOk
    );
    // SAFETY: inspect bytes within the full five-u16 backing buffer.
    let partial_bytes =
        unsafe { std::slice::from_raw_parts(partial_nul.as_ptr().cast::<u8>(), 10) };
    assert_eq!(&partial_bytes[..8], &expected_bytes[..8]);
    assert_eq!(partial_bytes[8], 0);
    assert_eq!(partial_bytes[9], 0x5a);
}

#[test]
fn message_binary_failure_clears_outputs() {
    let (_message, attributes) = create_message_and_attributes();
    assert_eq!(
        unsafe { attributes.setInt(c"integer".as_ptr(), 3) },
        kResultOk
    );
    for id in [c"missing", c"integer"] {
        let marker = 1_u8;
        let mut binary = (&raw const marker).cast::<std::ffi::c_void>();
        let mut binary_size = 12;
        assert_eq!(
            unsafe { attributes.getBinary(id.as_ptr(), &raw mut binary, &raw mut binary_size) },
            kResultFalse
        );
        assert!(binary.is_null());
        assert_eq!(binary_size, 0);
    }
}

#[test]
fn host_application_creates_standalone_attribute_lists() {
    let application = ComWrapper::new(HostApplication);
    let host = application
        .to_com_ptr::<IHostApplication>()
        .expect("host app");
    let mut class_id = IAttributeList_iid;
    let mut interface_id = IAttributeList_iid;
    let mut object = ptr::null_mut();
    assert_eq!(
        unsafe { host.createInstance(&raw mut class_id, &raw mut interface_id, &raw mut object) },
        kResultOk
    );
    let _attributes = unsafe { ComPtr::<IAttributeList>::from_raw(object.cast()) }
        .expect("host-created attribute list");
}

#[test]
fn creator_thread_boundary_is_checked_without_native_calls() {
    let services = Arc::new(HostServices::new());
    assert!(services.ensure_creator_thread().is_ok());
    let other_thread = Arc::clone(&services);
    let result = std::thread::spawn(move || other_thread.ensure_creator_thread())
        .join()
        .expect("thread completes");
    assert!(result.is_err());
}

#[test]
fn component_and_separate_controller_initialization_receive_retained_context() {
    let services = HostServices::new();
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let terminations = Arc::new(AtomicUsize::new(0));
    let component = ComWrapper::new(ProbeBase {
        contexts: Arc::clone(&contexts),
        terminations: Arc::clone(&terminations),
    });
    let controller = ComWrapper::new(ProbeBase {
        contexts: Arc::clone(&contexts),
        terminations: Arc::clone(&terminations),
    });
    let component_ptr = component
        .to_com_ptr::<IPluginBase>()
        .expect("component base");
    let controller_ptr = controller
        .to_com_ptr::<IPluginBase>()
        .expect("controller base");

    let component_guard =
        crate::lifecycle::InitializedPlugin::initialize(&component_ptr, services.context_ptr())
            .expect("component initializes");
    let controller_guard =
        crate::lifecycle::InitializedPlugin::initialize(&controller_ptr, services.context_ptr())
            .expect("separate controller initializes");
    let seen = contexts.lock().expect("probe lock");
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0], services.context_ptr().addr());
    assert_eq!(seen[1], services.context_ptr().addr());
    drop(seen);
    drop(controller_guard);
    drop(component_guard);
    assert_eq!(terminations.load(Ordering::Relaxed), 2);
}

#[test]
fn handler_coalesces_edits_and_preserves_callbacks_arriving_during_service() {
    let state = Arc::new(CallbackState::new());
    let handler = ComWrapper::new(ComponentHandler {
        state: Arc::clone(&state),
    });
    let pointer = handler
        .to_com_ptr::<IComponentHandler>()
        .expect("component handler");
    assert_eq!(unsafe { pointer.performEdit(17, 0.2) }, kResultOk);
    assert_eq!(unsafe { pointer.performEdit(17, 0.9) }, kResultOk);
    assert_eq!(unsafe { pointer.restartComponent(8) }, kResultOk);
    assert!(state.edits.has_pending());
    let serviced_flags = state.restart_flags.swap(0, Ordering::AcqRel);

    let mut observed = Vec::new();
    state
        .edits
        .service(|id, value| {
            observed.push((id, value));
            if id == 17 {
                assert_eq!(state.perform_edit(23, 0.4), kResultOk);
                assert_eq!(state.restart(4), kResultOk);
            }
            true
        })
        .expect("service current bank");
    assert_eq!(observed, [(17, 0.9)]);
    assert_eq!(serviced_flags, 8);
    assert_eq!(state.restart_flags.load(Ordering::Acquire), 4);
    assert!(state.edits.has_pending());

    let mut next = Vec::new();
    state
        .edits
        .service(|id, value| {
            next.push((id, value));
            true
        })
        .expect("service next bank");
    assert_eq!(next, [(23, 0.4)]);
}

#[test]
fn registered_callback_writer_cannot_be_drained_or_reset_until_it_exits() {
    let state = CallbackState::new();
    let index = state.edits.active.load(Ordering::SeqCst);
    let bank = &state.edits.banks[index];
    bank.writers.fetch_add(1, Ordering::SeqCst);
    assert_eq!(state.edits.active.load(Ordering::SeqCst), index);

    state
        .edits
        .rotate(&mut |_, _| panic!("registered writer bank was drained early"))
        .expect("rotate while writer is registered");
    assert!(bank.closed.load(Ordering::SeqCst));
    assert_eq!(bank.count.load(Ordering::Acquire), 0);

    assert!(bank.insert(41, 0.75));
    bank.writers.fetch_sub(1, Ordering::SeqCst);
    let mut observed = Vec::new();
    state
        .edits
        .service(|id, value| {
            observed.push((id, value));
            true
        })
        .expect("drain after the registered writer exits");
    assert_eq!(observed, [(41, 0.75)]);
}

fn publish_and_record(state: &CallbackState, accepted: &Mutex<Vec<u32>>, id: u32, value: f64) {
    if state.perform_edit(id, value) == kResultOk {
        accepted.lock().expect("accepted edits").push(id);
    }
}

struct ServiceRendezvous {
    writer_ready: Receiver<()>,
    service_ack: SyncSender<()>,
    callback_done: Receiver<()>,
    applied: Arc<Mutex<Vec<u32>>>,
    primed_edit_seen: bool,
}

impl ServiceRendezvous {
    fn apply(&mut self, id: u32) -> bool {
        self.applied.lock().expect("applied edits").push(id);
        if id == 0 && !self.primed_edit_seen {
            self.writer_ready
                .recv_timeout(Duration::from_secs(5))
                .expect("callback reaches active bank while service drains primed edit");
            self.service_ack
                .send(())
                .expect("callback receiver remains live");
            self.callback_done
                .recv_timeout(Duration::from_secs(5))
                .expect("callback completes during service pass");
            self.primed_edit_seen = true;
        }
        true
    }
}

#[test]
fn concurrent_callback_overlaps_service_without_losing_accepted_edits() {
    const CALLBACKS: u32 = 256;

    let state = Arc::new(CallbackState::new());
    assert_eq!(state.perform_edit(0, 0.5), kResultOk);
    let start = Arc::new(Barrier::new(3));
    let accepted = Arc::new(Mutex::new(vec![0_u32]));
    let applied = Arc::new(Mutex::new(Vec::new()));
    let (writer_ready, service_waiting) = sync_channel(1);
    let (service_ack, callback_waiting) = sync_channel(1);
    let (callback_done, service_receiving) = sync_channel(1);

    let callback_state = Arc::clone(&state);
    let callback_start = Arc::clone(&start);
    let callback_accepted = Arc::clone(&accepted);
    let service_applied = Arc::clone(&applied);
    let callback_thread = std::thread::spawn(move || {
        callback_start.wait();
        publish_and_record(&callback_state, &callback_accepted, 1, 0.25);
        writer_ready.send(()).expect("service thread remains live");
        callback_waiting
            .recv_timeout(Duration::from_secs(5))
            .expect("service observes primed edit");
        publish_and_record(&callback_state, &callback_accepted, 10_000, 0.75);
        callback_done
            .send(())
            .expect("service receiver remains live");
        for id in 2..=CALLBACKS {
            publish_and_record(
                &callback_state,
                &callback_accepted,
                id,
                f64::from(id) / f64::from(CALLBACKS),
            );
        }
    });

    let service_state = Arc::clone(&state);
    let service_start = Arc::clone(&start);
    let service_thread = std::thread::spawn(move || {
        service_start.wait();
        let mut rendezvous = ServiceRendezvous {
            writer_ready: service_waiting,
            service_ack,
            callback_done: service_receiving,
            applied: service_applied,
            primed_edit_seen: false,
        };
        for _ in 0..=CALLBACKS {
            service_state
                .edits
                .service(|id, _| rendezvous.apply(id))
                .expect("service callback bank");
        }
        assert!(rendezvous.primed_edit_seen);
    });

    start.wait();
    callback_thread.join().expect("callback thread completes");
    service_thread.join().expect("service thread completes");
    for _ in 0..2 {
        state
            .edits
            .service(|id, _| {
                applied.lock().expect("applied edits").push(id);
                true
            })
            .expect("final service drain");
    }

    let mut accepted = accepted.lock().expect("accepted edits").clone();
    let mut applied = applied.lock().expect("applied edits").clone();
    accepted.sort_unstable();
    applied.sort_unstable();
    assert_eq!(applied, accepted);
}

struct ControllerProbe {
    events: Arc<Mutex<Vec<&'static str>>>,
    terminations: Arc<AtomicUsize>,
    fail_handler: Cell<bool>,
    handler: RefCell<Option<vst3::ComPtr<IComponentHandler>>>,
}

impl Class for ControllerProbe {
    type Interfaces = (IEditController, vst3::Steinberg::Vst::IComponent);
}

impl vst3::Steinberg::Vst::IComponentTrait for ControllerProbe {
    unsafe fn getControllerClassId(&self, _: *mut vst3::Steinberg::TUID) -> i32 {
        panic!("a combined controller must not be asked for a separate controller class")
    }
    unsafe fn setIoMode(&self, _: i32) -> i32 {
        kResultOk
    }
    unsafe fn getBusCount(&self, _: i32, _: i32) -> i32 {
        0
    }
    unsafe fn getBusInfo(
        &self,
        _: i32,
        _: i32,
        _: i32,
        _: *mut vst3::Steinberg::Vst::BusInfo,
    ) -> i32 {
        kInvalidArgument
    }
    unsafe fn getRoutingInfo(
        &self,
        _: *mut vst3::Steinberg::Vst::RoutingInfo,
        _: *mut vst3::Steinberg::Vst::RoutingInfo,
    ) -> i32 {
        kInvalidArgument
    }
    unsafe fn activateBus(&self, _: i32, _: i32, _: i32, _: u8) -> i32 {
        kResultOk
    }
    unsafe fn setActive(&self, _: u8) -> i32 {
        kResultOk
    }
    unsafe fn setState(&self, _: *mut IBStream) -> i32 {
        kResultOk
    }
    unsafe fn getState(&self, _: *mut IBStream) -> i32 {
        kResultOk
    }
}

struct UnusedFactory;
impl Class for UnusedFactory {
    type Interfaces = (vst3::Steinberg::IPluginFactory,);
}
impl vst3::Steinberg::IPluginFactoryTrait for UnusedFactory {
    unsafe fn getFactoryInfo(&self, _: *mut vst3::Steinberg::PFactoryInfo) -> i32 {
        panic!("unused factory")
    }
    unsafe fn countClasses(&self) -> i32 {
        panic!("unused factory")
    }
    unsafe fn getClassInfo(&self, _: i32, _: *mut vst3::Steinberg::PClassInfo) -> i32 {
        panic!("unused factory")
    }
    unsafe fn createInstance(
        &self,
        _: *const std::ffi::c_char,
        _: *const std::ffi::c_char,
        _: *mut *mut std::ffi::c_void,
    ) -> i32 {
        panic!("combined controller must not create a separate instance")
    }
}

#[test]
fn combined_component_load_uses_its_existing_controller_without_reinitializing() {
    let ControllerFixture {
        controller, events, ..
    } = controller_probe(false);
    let component = controller
        .to_com_ptr::<vst3::Steinberg::Vst::IComponent>()
        .unwrap();
    let services = HostServices::new();
    let base = component.as_com_ref().cast::<IPluginBase>().unwrap();
    let _lifecycle =
        crate::lifecycle::InitializedPlugin::initialize(&base, services.context_ptr()).unwrap();
    let factory = ComWrapper::new(UnusedFactory)
        .to_com_ptr::<vst3::Steinberg::IPluginFactory>()
        .unwrap();
    let info = truce_rack_core::info::PluginInfo {
        name: "Combined".into(),
        vendor: "Fixture".into(),
        version: 1,
        category: truce_rack_core::info::PluginCategory::Effect,
        path: "combined.vst3".into(),
        unique_id: "fixture".into(),
        format: "vst3",
        has_editor: false,
        accepts_midi: false,
    };
    let context = crate::loading::LoadContext {
        info: &info,
        factory: &factory,
        host_services: &services,
    };
    let parts = crate::loading::load_controller(&context, &component).unwrap();
    assert!(!parts.separate);
    assert!(parts.lifecycle.is_none());
    assert_eq!(*events.lock().unwrap(), ["initialize"]);
}

impl IPluginBaseTrait for ControllerProbe {
    unsafe fn initialize(&self, context: *mut FUnknown) -> i32 {
        if context.is_null() {
            return kInvalidArgument;
        }
        self.events
            .lock()
            .expect("controller event lock")
            .push("initialize");
        kResultOk
    }

    unsafe fn terminate(&self) -> i32 {
        self.events
            .lock()
            .expect("controller event lock")
            .push("terminate");
        if let Some(handler) = self.handler.borrow().as_ref() {
            assert_eq!(unsafe { handler.endEdit(7) }, kResultOk);
        }
        self.terminations.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
}

impl IEditControllerTrait for ControllerProbe {
    unsafe fn setComponentState(&self, _: *mut IBStream) -> i32 {
        kInvalidArgument
    }
    unsafe fn setState(&self, _: *mut IBStream) -> i32 {
        kInvalidArgument
    }
    unsafe fn getState(&self, _: *mut IBStream) -> i32 {
        kInvalidArgument
    }
    unsafe fn getParameterCount(&self) -> i32 {
        0
    }
    unsafe fn getParameterInfo(&self, _: i32, _: *mut ParameterInfo) -> i32 {
        kInvalidArgument
    }
    unsafe fn getParamStringByValue(&self, _: u32, _: f64, _: *mut [u16; 128]) -> i32 {
        kInvalidArgument
    }
    unsafe fn getParamValueByString(&self, _: u32, _: *mut u16, _: *mut f64) -> i32 {
        kInvalidArgument
    }
    unsafe fn normalizedParamToPlain(&self, _: u32, value: f64) -> f64 {
        value
    }
    unsafe fn plainParamToNormalized(&self, _: u32, value: f64) -> f64 {
        value
    }
    unsafe fn getParamNormalized(&self, _: u32) -> f64 {
        0.0
    }
    unsafe fn setParamNormalized(&self, _: u32, _: f64) -> i32 {
        kResultOk
    }

    unsafe fn setComponentHandler(&self, raw: *mut IComponentHandler) -> i32 {
        self.events
            .lock()
            .expect("controller event lock")
            .push("handler");
        if self.fail_handler.get() {
            return kInvalidArgument;
        }
        let Some(handler) = (unsafe { ComRef::<IComponentHandler>::from_raw(raw) }) else {
            return kInvalidArgument;
        };
        *self.handler.borrow_mut() = Some(handler.to_com_ptr());
        kResultOk
    }

    unsafe fn createView(&self, _: *const std::ffi::c_char) -> *mut IPlugView {
        self.events
            .lock()
            .expect("controller event lock")
            .push("view");
        std::ptr::null_mut()
    }
}

struct ControllerFixture {
    controller: ComWrapper<ControllerProbe>,
    events: Arc<Mutex<Vec<&'static str>>>,
    terminations: Arc<AtomicUsize>,
}

fn controller_probe(fail_handler: bool) -> ControllerFixture {
    let events = Arc::new(Mutex::new(Vec::new()));
    let terminations = Arc::new(AtomicUsize::new(0));
    let controller = ComWrapper::new(ControllerProbe {
        events: Arc::clone(&events),
        terminations: Arc::clone(&terminations),
        fail_handler: Cell::new(fail_handler),
        handler: RefCell::new(None),
    });
    ControllerFixture {
        controller,
        events,
        terminations,
    }
}

#[test]
fn controller_is_initialized_then_attached_and_handler_survives_until_terminate() {
    let services = HostServices::new();
    let ControllerFixture {
        controller,
        events,
        terminations,
    } = controller_probe(false);
    let controller_ptr = controller
        .to_com_ptr::<IEditController>()
        .expect("controller");
    let base = controller_ptr
        .as_com_ref()
        .cast::<IPluginBase>()
        .expect("base");
    let guard = services
        .initialize_separate_controller(&base, &controller_ptr)
        .expect("initialized and attached");
    assert_eq!(
        *events.lock().expect("controller event lock"),
        ["initialize", "handler"]
    );
    unsafe { controller_ptr.createView(c"editor".as_ptr()) };
    drop(guard);
    assert_eq!(
        *events.lock().expect("controller event lock"),
        ["initialize", "handler", "view", "terminate"]
    );
    assert_eq!(terminations.load(Ordering::Relaxed), 1);
}

#[test]
fn failed_handler_attachment_terminates_the_initialized_controller() {
    let services = HostServices::new();
    let ControllerFixture {
        controller,
        events,
        terminations,
    } = controller_probe(true);
    let controller_ptr = controller
        .to_com_ptr::<IEditController>()
        .expect("controller");
    let base = controller_ptr
        .as_com_ref()
        .cast::<IPluginBase>()
        .expect("base");
    assert!(
        services
            .initialize_separate_controller(&base, &controller_ptr)
            .is_err()
    );
    assert_eq!(
        *events.lock().expect("controller event lock"),
        ["initialize", "handler", "terminate"]
    );
    assert_eq!(terminations.load(Ordering::Relaxed), 1);
}

#[test]
fn callback_payload_is_bounded_allocation_free_and_invalid_input_fails_closed() {
    let state = Arc::new(CallbackState::new());
    let handler = ComWrapper::new(ComponentHandler {
        state: Arc::clone(&state),
    });
    let pointer = handler
        .to_com_ptr::<IComponentHandler>()
        .expect("component handler");
    let allocations = crate::parameter_delivery::count_allocations(|| {
        let capacity = u32::try_from(EDIT_CAPACITY).expect("small fixed capacity");
        for id in 0..capacity {
            assert_eq!(unsafe { pointer.performEdit(id, 0.5) }, kResultOk);
        }
        assert_eq!(
            unsafe { pointer.performEdit(capacity, 0.6) },
            kInvalidArgument
        );
        assert_eq!(
            unsafe { pointer.performEdit(1, f64::NAN) },
            kInvalidArgument
        );
        assert_eq!(unsafe { pointer.restartComponent(8) }, kResultOk);
    });
    assert_eq!(allocations, 0);
    assert!(state.failed.load(Ordering::Acquire));
    assert_eq!(state.restart_flags.load(Ordering::Acquire), 8);
    assert_eq!(unsafe { pointer.restartComponent(-1) }, kInvalidArgument);
    assert!(state.failed.load(Ordering::Acquire));
}

#[test]
fn native_value_invalidation_is_accepted_without_outbound_edits_or_restart() {
    let state = CallbackState::new();
    let flag = RestartFlags_::kParamValuesChanged as u32;
    assert_eq!(state.restart(i32::try_from(flag).unwrap()), kResultOk);
    let requests = parse_restart_flags(state.restart_flags.swap(0, Ordering::AcqRel))
        .expect("native value invalidation is supported");
    assert!(!requests.latency);
    assert!(!requests.parameter_titles);
    assert!(!state.edits.has_pending());
}

#[test]
fn restart_requests_are_coalesced_and_unimplemented_reconfiguration_fails_closed() {
    let supported = RestartFlags_::kLatencyChanged as u32
        | RestartFlags_::kParamTitlesChanged as u32
        | RestartFlags_::kParamValuesChanged as u32;
    let requests = parse_restart_flags(supported).expect("supported restart set");
    assert!(requests.latency);
    assert!(requests.parameter_titles);
    assert!(parse_restart_flags(RestartFlags_::kParamIDMappingChanged as u32).is_err());
    assert!(parse_restart_flags(RestartFlags_::kIoChanged as u32).is_err());
}

#[test]
fn latency_snapshot_revision_stays_unknown_after_a_new_callback_during_refresh() {
    let state = CallbackState::new();
    let first_revision = state.latency_revision.load(Ordering::Acquire);
    state
        .latency_snapshot_revision
        .store(first_revision, Ordering::Release);
    assert_eq!(
        state.latency_revision.load(Ordering::Acquire),
        state.latency_snapshot_revision.load(Ordering::Acquire)
    );

    state.restart(RestartFlags_::kLatencyChanged);
    let refresh_revision = state.latency_revision.load(Ordering::Acquire);
    state
        .latency_snapshot_revision
        .store(first_revision, Ordering::Release);
    assert_ne!(
        state.latency_revision.load(Ordering::Acquire),
        state.latency_snapshot_revision.load(Ordering::Acquire)
    );
    state
        .latency_snapshot_revision
        .store(refresh_revision, Ordering::Release);
    assert_eq!(
        state.latency_revision.load(Ordering::Acquire),
        state.latency_snapshot_revision.load(Ordering::Acquire)
    );

    assert_eq!(state.perform_edit(9, 0.25), kResultOk);
    assert_ne!(
        state.latency_revision.load(Ordering::Acquire),
        state.latency_snapshot_revision.load(Ordering::Acquire)
    );
}

#[test]
fn lifecycle_guard_terminates_partial_owner_and_moves_into_complete_owner() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let terminations = Arc::new(AtomicUsize::new(0));
    let plugin = ComWrapper::new(ProbeBase {
        contexts,
        terminations: Arc::clone(&terminations),
    });
    let pointer = plugin.to_com_ptr::<IPluginBase>().expect("plugin base");
    {
        let _guard =
            crate::lifecycle::InitializedPlugin::initialize(&pointer, std::ptr::null_mut())
                .expect("initialized");
    }
    assert_eq!(terminations.load(Ordering::Relaxed), 1);
    {
        let guard = crate::lifecycle::InitializedPlugin::initialize(&pointer, std::ptr::null_mut())
            .expect("initialized");
        let _owner = crate::lifecycle::PluginLifetimes::new(guard, None);
    }
    assert_eq!(terminations.load(Ordering::Relaxed), 2);
}
