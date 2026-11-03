use super::*;
use std::cell::{Cell, RefCell};
use vst3::{
    Class, ComWrapper,
    Steinberg::{
        FUnknown, IBStream, IPluginBaseTrait, TUID,
        Vst::{
            BusInfo, IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentTrait,
            ProcessData, ProcessSetup, RoutingInfo,
        },
        kInvalidArgument,
    },
};

const AUDIO: i32 = 0;
const INPUT: i32 = 0;
const OUTPUT: i32 = 1;
const LATENCY_BEFORE_ACTIVE: u32 = 19;
const LATENCY_AFTER_ACTIVE: u32 = 137;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    BusArrangements,
    SetupProcessing,
    ActivateBus(i32, i32, u8),
    SetActive(u8),
    GetLatency,
    SetProcessing(u8),
    Terminate,
}

struct Native {
    calls: RefCell<Vec<Call>>,
    input_mask: Cell<u64>,
    output_mask: Cell<u64>,
    latency: Cell<u32>,
    fail_active: Cell<bool>,
    fail_processing: Cell<bool>,
    fail_stop_once: Cell<bool>,
}

impl Native {
    fn new(fail_active: bool, fail_processing: bool) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            input_mask: Cell::new(1),
            output_mask: Cell::new(1),
            latency: Cell::new(LATENCY_BEFORE_ACTIVE),
            fail_active: Cell::new(fail_active),
            fail_processing: Cell::new(fail_processing),
            fail_stop_once: Cell::new(false),
        }
    }

    fn mask(&self, direction: i32) -> u64 {
        if direction == INPUT {
            self.input_mask.get()
        } else {
            self.output_mask.get()
        }
    }
}

impl Class for Native {
    type Interfaces = (vst3::Steinberg::Vst::IComponent, IAudioProcessor);
}

impl IPluginBaseTrait for Native {
    unsafe fn initialize(&self, _: *mut FUnknown) -> i32 {
        vst3::Steinberg::kResultOk
    }

    unsafe fn terminate(&self) -> i32 {
        self.calls.borrow_mut().push(Call::Terminate);
        vst3::Steinberg::kResultOk
    }
}

impl IComponentTrait for Native {
    unsafe fn getControllerClassId(&self, _: *mut TUID) -> i32 {
        kInvalidArgument
    }

    unsafe fn setIoMode(&self, _: i32) -> i32 {
        vst3::Steinberg::kResultOk
    }

    unsafe fn getBusCount(&self, media_type: i32, direction: i32) -> i32 {
        i32::from(media_type == AUDIO && (direction == INPUT || direction == OUTPUT))
    }

    unsafe fn getBusInfo(
        &self,
        media_type: i32,
        direction: i32,
        index: i32,
        info: *mut BusInfo,
    ) -> i32 {
        if media_type != AUDIO
            || (direction != INPUT && direction != OUTPUT)
            || index != 0
            || info.is_null()
        {
            return kInvalidArgument;
        }
        // SAFETY: the VST3 caller supplies one writable BusInfo for valid bus zero.
        unsafe {
            *info = BusInfo {
                mediaType: AUDIO,
                direction,
                channelCount: 1,
                name: [0; 128],
                busType: 0,
                flags: 0,
            };
        }
        vst3::Steinberg::kResultOk
    }

    unsafe fn getRoutingInfo(&self, _: *mut RoutingInfo, _: *mut RoutingInfo) -> i32 {
        kInvalidArgument
    }

    unsafe fn activateBus(&self, _: i32, direction: i32, index: i32, state: u8) -> i32 {
        self.calls
            .borrow_mut()
            .push(Call::ActivateBus(direction, index, state));
        vst3::Steinberg::kResultOk
    }

    unsafe fn setActive(&self, state: u8) -> i32 {
        self.calls.borrow_mut().push(Call::SetActive(state));
        if state != 0 {
            // Model a plug-in that selects its actual latency during activation.
            self.latency.set(LATENCY_AFTER_ACTIVE);
            if self.fail_active.get() {
                return kInvalidArgument;
            }
        }
        vst3::Steinberg::kResultOk
    }

    unsafe fn setState(&self, _: *mut IBStream) -> i32 {
        vst3::Steinberg::kResultOk
    }

    unsafe fn getState(&self, _: *mut IBStream) -> i32 {
        vst3::Steinberg::kResultOk
    }
}

impl IAudioProcessorTrait for Native {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut u64,
        num_inputs: i32,
        outputs: *mut u64,
        num_outputs: i32,
    ) -> i32 {
        self.calls.borrow_mut().push(Call::BusArrangements);
        if num_inputs != 1 || num_outputs != 1 || inputs.is_null() || outputs.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: checked non-null pointers each address one arrangement per the counts above.
        self.input_mask.set(unsafe { *inputs });
        self.output_mask.set(unsafe { *outputs });
        vst3::Steinberg::kResultOk
    }

    unsafe fn getBusArrangement(&self, direction: i32, index: i32, arrangement: *mut u64) -> i32 {
        if index != 0 || (direction != INPUT && direction != OUTPUT) || arrangement.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: checked non-null output pointer addresses one arrangement.
        unsafe { *arrangement = self.mask(direction) };
        vst3::Steinberg::kResultOk
    }

    unsafe fn canProcessSampleSize(&self, _: i32) -> i32 {
        vst3::Steinberg::kResultOk
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        self.calls.borrow_mut().push(Call::GetLatency);
        self.latency.get()
    }

    unsafe fn setupProcessing(&self, _: *mut ProcessSetup) -> i32 {
        self.calls.borrow_mut().push(Call::SetupProcessing);
        vst3::Steinberg::kResultOk
    }

    unsafe fn setProcessing(&self, state: u8) -> i32 {
        self.calls.borrow_mut().push(Call::SetProcessing(state));
        if (state == 0 && self.fail_stop_once.replace(false))
            || (state != 0 && self.fail_processing.get())
        {
            kInvalidArgument
        } else {
            vst3::Steinberg::kResultOk
        }
    }

    unsafe fn process(&self, _: *mut ProcessData) -> i32 {
        vst3::Steinberg::kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

fn process_setup() -> ProcessSetup {
    ProcessSetup {
        processMode: 0,
        symbolicSampleSize: 0,
        maxSamplesPerBlock: 64,
        sampleRate: 48_000.0,
    }
}

fn discovered_layout(native: &ComWrapper<Native>) -> NativeLayout {
    let component = native
        .to_com_ptr::<IComponent>()
        .expect("component interface");
    let processor = native
        .to_com_ptr::<IAudioProcessor>()
        .expect("processor interface");
    let layout = bus_layout::discover(&component, &processor)
        .expect("native exact topology")
        .into_iter()
        .next()
        .expect("full topology candidate");
    native.calls.borrow_mut().clear();
    layout
}

fn position(calls: &[Call], call: Call) -> usize {
    calls
        .iter()
        .position(|entry| *entry == call)
        .expect("expected native call")
}

fn deactivated_bus_calls(calls: &[Call]) -> usize {
    calls
        .iter()
        .filter(|call| matches!(call, Call::ActivateBus(_, _, 0)))
        .count()
}

#[test]
fn latency_is_read_after_activation_and_cached_only_after_processing_starts() {
    let native = ComWrapper::new(Native::new(false, false));
    let component = native.to_com_ptr().expect("component interface");
    let processor = native.to_com_ptr().expect("processor interface");
    let layout = discovered_layout(&native);
    let mut setup = process_setup();
    let mut latency = Some(7);

    activate(&component, &processor, &layout, &mut setup, &mut latency).expect("activation");

    assert_eq!(latency, Some(LATENCY_AFTER_ACTIVE));
    let calls = native.calls.borrow();
    assert!(
        position(&calls, Call::SetActive(1)) < position(&calls, Call::GetLatency),
        "latency must be queried after setActive(true): {calls:?}"
    );
    assert!(
        position(&calls, Call::GetLatency) < position(&calls, Call::SetProcessing(1)),
        "latency must be queried before setProcessing(true): {calls:?}"
    );
}

#[test]
fn latency_restart_stops_deactivates_then_reactivates_before_publishing_new_snapshot() {
    let native = ComWrapper::new(Native::new(false, false));
    let component = native.to_com_ptr().expect("component interface");
    let processor = native.to_com_ptr().expect("processor interface");
    let layout = discovered_layout(&native);
    let mut setup = process_setup();
    let mut latency = None;
    activate(&component, &processor, &layout, &mut setup, &mut latency)
        .expect("initial activation");
    native.calls.borrow_mut().clear();
    let mut processing = true;

    restart(
        &component,
        &processor,
        &layout,
        &mut RestartState {
            setup: &mut setup,
            processing: &mut processing,
            latency_frames: &mut latency,
        },
    )
    .expect("latency restart");

    assert!(processing);
    assert_eq!(latency, Some(LATENCY_AFTER_ACTIVE));
    let calls = native.calls.borrow();
    assert!(position(&calls, Call::SetProcessing(0)) < position(&calls, Call::SetActive(0)));
    assert!(position(&calls, Call::SetActive(0)) < position(&calls, Call::SetupProcessing));
    assert!(position(&calls, Call::SetupProcessing) < position(&calls, Call::SetActive(1)));
    assert!(position(&calls, Call::SetActive(1)) < position(&calls, Call::GetLatency));
    assert!(position(&calls, Call::GetLatency) < position(&calls, Call::SetProcessing(1)));
}

#[test]
fn drop_cleanup_retries_deactivation_and_disables_buses_before_terminate() {
    let native = ComWrapper::new(Native::new(false, false));
    let component = native.to_com_ptr().expect("component interface");
    let processor = native.to_com_ptr().expect("processor interface");
    let layout = discovered_layout(&native);
    let mut setup = process_setup();
    let mut latency = None;
    activate(&component, &processor, &layout, &mut setup, &mut latency)
        .expect("initial activation");
    native.calls.borrow_mut().clear();

    let mut processing = true;
    native.fail_stop_once.set(true);
    assert!(deactivate(&component, &processor, &layout, &mut processing).is_err());
    assert!(processing, "failed native stop must remain retryable");

    super::deactivate_before_terminate(&component, &processor, Some(&layout), &mut processing);
    assert!(!processing);
    assert_eq!(unsafe { component.terminate() }, vst3::Steinberg::kResultOk);

    let calls = native.calls.borrow();
    let termination = position(&calls, Call::Terminate);
    let bus_states = calls[..termination]
        .iter()
        .filter_map(|call| match call {
            Call::ActivateBus(_, _, state) => Some(*state),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(bus_states, [0, 0, 0, 0]);
    assert!(
        calls[..termination]
            .iter()
            .any(|call| matches!(call, Call::SetProcessing(0)))
    );
}

#[test]
fn failed_activation_never_queries_or_publishes_latency_and_deactivates_buses() {
    let native = ComWrapper::new(Native::new(true, false));
    let component = native.to_com_ptr().expect("component interface");
    let processor = native.to_com_ptr().expect("processor interface");
    let layout = discovered_layout(&native);
    let mut setup = process_setup();
    let mut latency = Some(7);

    assert!(activate(&component, &processor, &layout, &mut setup, &mut latency,).is_err());

    assert_eq!(latency, None);
    let calls = native.calls.borrow();
    assert!(!calls.contains(&Call::GetLatency));
    assert_eq!(deactivated_bus_calls(&calls), 2);
}

#[test]
fn processing_failure_after_latency_read_keeps_cache_empty_and_rolls_back_buses() {
    let native = ComWrapper::new(Native::new(false, true));
    let component = native.to_com_ptr().expect("component interface");
    let processor = native.to_com_ptr().expect("processor interface");
    let layout = discovered_layout(&native);
    let mut setup = process_setup();
    let mut latency = Some(7);

    assert!(activate(&component, &processor, &layout, &mut setup, &mut latency,).is_err());

    assert_eq!(latency, None);
    let calls = native.calls.borrow();
    assert!(position(&calls, Call::SetActive(1)) < position(&calls, Call::GetLatency));
    assert!(position(&calls, Call::GetLatency) < position(&calls, Call::SetProcessing(1)));
    assert!(calls.contains(&Call::SetActive(0)));
    assert!(calls.contains(&Call::SetProcessing(0)));
    assert_eq!(deactivated_bus_calls(&calls), 2);
}
