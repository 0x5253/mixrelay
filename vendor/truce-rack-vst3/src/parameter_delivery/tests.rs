use super::*;
use std::cell::{Cell, RefCell};
use vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, IComponentHandler, ParameterInfo, ProcessSetup,
};
use vst3::Steinberg::{FUnknown, IBStream, IPlugView, IPluginBaseTrait};

use crate::test_support::count_allocations;

struct Controller {
    values: RefCell<[f64; 2]>,
    reject: Cell<bool>,
    duplicate_ids: Cell<bool>,
}
impl Controller {
    fn new() -> Self {
        Self {
            values: RefCell::new([1.0; 2]),
            reject: Cell::new(false),
            duplicate_ids: Cell::new(false),
        }
    }
}
impl Class for Controller {
    type Interfaces = (IEditController,);
}
impl IPluginBaseTrait for Controller {
    unsafe fn initialize(&self, _: *mut FUnknown) -> i32 {
        kResultOk
    }
    unsafe fn terminate(&self) -> i32 {
        kResultOk
    }
}
impl IEditControllerTrait for Controller {
    unsafe fn setComponentState(&self, _: *mut IBStream) -> i32 {
        kResultOk
    }
    unsafe fn setState(&self, _: *mut IBStream) -> i32 {
        kResultOk
    }
    unsafe fn getState(&self, _: *mut IBStream) -> i32 {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> i32 {
        2
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> i32 {
        if info.is_null() || !(0..2).contains(&index) {
            return kInvalidArgument;
        }
        let id = if index == 0 || self.duplicate_ids.get() {
            7
        } else {
            9001
        };
        unsafe {
            *info = crate::empty_parameter_info();
            (*info).id = id;
        }
        kResultOk
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
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        self.values.borrow()[usize::from(id == 9001)]
    }
    unsafe fn setParamNormalized(&self, id: u32, value: f64) -> i32 {
        if self.reject.get() {
            return kInvalidArgument;
        }
        self.values.borrow_mut()[usize::from(id == 9001)] = value;
        kResultOk
    }
    unsafe fn setComponentHandler(&self, _: *mut IComponentHandler) -> i32 {
        kResultOk
    }
    unsafe fn createView(&self, _: *const std::ffi::c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

// This COM object deliberately has no reference to Controller. Its audible gain
// can change only through ProcessData.inputParameterChanges, like a separate DSP.
struct Processor {
    values: RefCell<[f64; 2]>,
    points: Cell<usize>,
    fail: Cell<bool>,
}
impl Processor {
    fn new() -> Self {
        Self {
            values: RefCell::new([1.0; 2]),
            points: Cell::new(0),
            fail: Cell::new(false),
        }
    }
}
impl Class for Processor {
    type Interfaces = (IAudioProcessor,);
}
impl IAudioProcessorTrait for Processor {
    unsafe fn setBusArrangements(&self, _: *mut u64, _: i32, _: *mut u64, _: i32) -> i32 {
        kResultOk
    }
    unsafe fn getBusArrangement(&self, _: i32, _: i32, _: *mut u64) -> i32 {
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, _: i32) -> i32 {
        kResultOk
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, _: *mut ProcessSetup) -> i32 {
        kResultOk
    }
    unsafe fn setProcessing(&self, _: u8) -> i32 {
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
    unsafe fn process(&self, data: *mut ProcessData) -> i32 {
        if self.fail.get() {
            return kInvalidArgument;
        }
        let data = unsafe { &mut *data };
        let changes =
            unsafe { vst3::ComRef::<IParameterChanges>::from_raw(data.inputParameterChanges) };
        let count = changes
            .as_ref()
            .map_or(0, |changes| unsafe { changes.getParameterCount() });
        self.points.set(usize::try_from(count).unwrap());
        for index in 0..count {
            let raw = unsafe { changes.as_ref().unwrap().getParameterData(index) };
            let queue = unsafe { vst3::ComRef::<IParamValueQueue>::from_raw(raw) }.unwrap();
            assert_eq!(unsafe { queue.getPointCount() }, 1);
            let mut offset = -1;
            let mut value = -1.0;
            assert_eq!(
                unsafe { queue.getPoint(0, &raw mut offset, &raw mut value) },
                kResultOk
            );
            assert_eq!(offset, 0);
            let id = unsafe { queue.getParameterId() };
            assert!(id == 7 || id == 9001);
            self.values.borrow_mut()[usize::from(id == 9001)] = value;
        }
        let input = unsafe { *(*(*data.inputs).__field0.channelBuffers32) };
        let output = unsafe { *(*data.outputs).__field0.channelBuffers32 };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "fixture renders normalized f64 control values into Float32 PCM"
        )]
        unsafe {
            *output = input * self.values.borrow()[0] as f32;
        }
        kResultOk
    }
}

fn run(parameters: &mut InputParameters, processor: &ComPtr<IAudioProcessor>) -> (i32, f32) {
    run_with_delivery(Some(parameters), processor)
}

fn run_with_delivery(
    parameters: Option<&mut InputParameters>,
    processor: &ComPtr<IAudioProcessor>,
) -> (i32, f32) {
    let mut input = 0.5_f32;
    let mut output = -1.0_f32;
    let mut input_ptr = ptr::from_mut(&mut input);
    let mut output_ptr = ptr::from_mut(&mut output);
    let mut input_bus = AudioBusBuffers {
        numChannels: 1,
        silenceFlags: 0,
        __field0: AudioBusBuffers__type0 {
            channelBuffers32: &raw mut input_ptr,
        },
    };
    let mut output_bus = AudioBusBuffers {
        numChannels: 1,
        silenceFlags: 0,
        __field0: AudioBusBuffers__type0 {
            channelBuffers32: &raw mut output_ptr,
        },
    };
    let mut data = ProcessData {
        processMode: 0,
        symbolicSampleSize: 0,
        numSamples: 1,
        numInputs: 1,
        numOutputs: 1,
        inputs: &raw mut input_bus,
        outputs: &raw mut output_bus,
        inputParameterChanges: ptr::null_mut(),
        outputParameterChanges: ptr::null_mut(),
        inputEvents: ptr::null_mut(),
        outputEvents: ptr::null_mut(),
        processContext: ptr::null_mut(),
    };
    let status = match parameters {
        Some(parameters) => parameters.process(processor, &mut data),
        None => unsafe { processor.process(&raw mut data) },
    };
    assert!(data.inputParameterChanges.is_null());
    (status, output)
}

#[test]
fn separate_controller_slider_edit_reaches_processor_pcm_and_is_consumed_once() {
    let controller = ComWrapper::new(Controller::new());
    let controller_ptr = controller.to_com_ptr::<IEditController>().unwrap();
    let processor = ComWrapper::new(Processor::new());
    let processor_ptr = processor.to_com_ptr::<IAudioProcessor>().unwrap();
    let mut parameters = InputParameters::from_controller(&controller_ptr, 2).unwrap();
    parameters.set(&controller_ptr, 0, 0.25).unwrap();
    assert_eq!(controller.values.borrow()[0].to_bits(), 0.25_f64.to_bits());
    assert_eq!(processor.values.borrow()[0].to_bits(), 1.0_f64.to_bits());
    // Same controller edit and 0.5 input: the old null-queue path leaves PCM
    // at 0.5. The production delivery path renders 0.125, proving DSP changed.
    assert_eq!(run_with_delivery(None, &processor_ptr), (kResultOk, 0.5));
    assert_eq!(run(&mut parameters, &processor_ptr), (kResultOk, 0.125));
    assert_eq!(processor.points.get(), 1);
    assert_eq!(run(&mut parameters, &processor_ptr), (kResultOk, 0.125));
    assert_eq!(processor.points.get(), 0);
}

#[test]
fn repeated_edits_coalesce_distinct_ids_stay_distinct_and_failure_retries() {
    let controller = ComWrapper::new(Controller::new());
    let controller_ptr = controller.to_com_ptr::<IEditController>().unwrap();
    let processor = ComWrapper::new(Processor::new());
    let processor_ptr = processor.to_com_ptr::<IAudioProcessor>().unwrap();
    let mut parameters = InputParameters::from_controller(&controller_ptr, 2).unwrap();
    for value in [0.1, 0.2, 0.3] {
        parameters.set(&controller_ptr, 0, value).unwrap();
    }
    parameters.set(&controller_ptr, 1, 0.8).unwrap();
    processor.fail.set(true);
    assert_eq!(run(&mut parameters, &processor_ptr).0, kInvalidArgument);
    processor.fail.set(false);
    assert_eq!(run(&mut parameters, &processor_ptr).0, kResultOk);
    assert_eq!(
        processor.values.borrow().map(f64::to_bits),
        [0.3_f64.to_bits(), 0.8_f64.to_bits()]
    );
    assert_eq!(processor.points.get(), 2);
}

#[test]
fn editor_values_queue_without_recursive_controller_setter_and_remain_pending_until_process() {
    let controller = ComWrapper::new(Controller::new());
    let pointer = controller.to_com_ptr::<IEditController>().unwrap();
    let processor = ComWrapper::new(Processor::new());
    let processor_pointer = processor.to_com_ptr::<IAudioProcessor>().unwrap();
    let mut parameters = InputParameters::from_controller(&pointer, 2).unwrap();
    controller.reject.set(true);

    parameters.queue_id_value(7, 0.375).unwrap();
    assert!(parameters.has_pending_changes());
    assert_eq!(controller.values.borrow()[0].to_bits(), 1.0_f64.to_bits());
    assert_eq!(
        run(&mut parameters, &processor_pointer),
        (kResultOk, 0.1875)
    );
    assert!(!parameters.has_pending_changes());
}

#[test]
fn metadata_refresh_rejects_id_remapping_without_queuing_edits() {
    let controller = ComWrapper::new(Controller::new());
    let pointer = controller.to_com_ptr::<IEditController>().unwrap();
    let mut parameters = InputParameters::from_controller(&pointer, 2).unwrap();
    controller.values.replace([0.25, 0.75]);
    parameters.refresh_metadata(&pointer).unwrap();
    assert!(!parameters.has_pending_changes());
    controller.duplicate_ids.set(true);
    assert!(parameters.refresh_metadata(&pointer).is_err());
    assert!(!parameters.has_pending_changes());
}

#[test]
fn rejected_edits_and_invalid_abi_access_never_publish() {
    let controller = ComWrapper::new(Controller::new());
    let pointer = controller.to_com_ptr::<IEditController>().unwrap();
    let mut parameters = InputParameters::from_controller(&pointer, 2).unwrap();
    assert!(parameters.set(&pointer, 0, f64::NAN).is_err());
    assert!(parameters.set(&pointer, 0, f64::INFINITY).is_err());
    assert!(parameters.set(&pointer, 2, 0.5).is_err());
    controller.reject.set(true);
    assert!(parameters.set(&pointer, 0, 0.5).is_err());
    assert_eq!(unsafe { parameters.pointer.getParameterCount() }, 0);
    assert!(unsafe { parameters.pointer.getParameterData(-1) }.is_null());
    assert!(unsafe { parameters.pointer.getParameterData(0) }.is_null());
    let queue = &parameters.changes.slots[0].pointer;
    assert_eq!(
        unsafe { queue.getPoint(0, ptr::null_mut(), ptr::null_mut()) },
        kInvalidArgument
    );
    controller.duplicate_ids.set(true);
    assert!(InputParameters::from_controller(&pointer, 2).is_err());
    assert!(InputParameters::from_controller(&pointer, MAX_PARAMETERS + 1).is_err());
}

#[test]
fn parameter_delivery_has_zero_allocations_for_ten_thousand_blocks() {
    let controller = ComWrapper::new(Controller::new());
    let pointer = controller.to_com_ptr::<IEditController>().unwrap();
    let processor = ComWrapper::new(Processor::new());
    let processor_pointer = processor.to_com_ptr::<IAudioProcessor>().unwrap();
    let mut parameters = InputParameters::from_controller(&pointer, 2).unwrap();
    let allocations = count_allocations(|| {
        for _ in 0..10_000 {
            parameters.set(&pointer, 0, 0.2).unwrap();
            parameters.set(&pointer, 0, 0.7).unwrap();
            parameters.set(&pointer, 1, 0.4).unwrap();
            assert_eq!(run(&mut parameters, &processor_pointer).0, kResultOk);
        }
    });
    assert_eq!(allocations, 0);
}

#[test]
fn retained_com_queue_survives_owner_drop_and_reads_safely_on_another_thread() {
    let controller = ComWrapper::new(Controller::new());
    let pointer = controller.to_com_ptr::<IEditController>().unwrap();
    let mut parameters = InputParameters::from_controller(&pointer, 2).unwrap();
    parameters.set(&pointer, 1, 0.625).unwrap();
    let changes = parameters.pointer.clone();
    let queue = unsafe { changes.getParameterData(0) };
    let queue = unsafe { vst3::ComRef::<IParamValueQueue>::from_raw(queue) }
        .unwrap()
        .to_com_ptr();
    drop(parameters);
    drop(changes);
    let value = std::thread::spawn(move || {
        assert_eq!(unsafe { queue.getParameterId() }, 9001);
        let mut offset = -1;
        let mut value = -1.0;
        assert_eq!(
            unsafe { queue.getPoint(0, &raw mut offset, &raw mut value) },
            kResultOk
        );
        assert_eq!(offset, 0);
        assert_eq!(
            unsafe { queue.getPoint(-1, &raw mut offset, &raw mut value) },
            kInvalidArgument
        );
        assert_eq!(
            unsafe { queue.addPoint(0, 0.2, &raw mut offset) },
            kInvalidArgument
        );
        value.to_bits()
    })
    .join()
    .unwrap();
    assert_eq!(value, 0.625_f64.to_bits());
}
