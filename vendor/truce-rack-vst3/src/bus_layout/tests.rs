use super::*;
use std::cell::{Cell, RefCell};
use truce_rack_core::buffer::AudioBuffer;
use truce_rack_core::buffer::BusRange;
use vst3::{
    Class, ComWrapper,
    Steinberg::{
        FUnknown, IBStream, IPluginBaseTrait, TUID,
        Vst::{ProcessData, ProcessSetup, RoutingInfo},
        kInvalidArgument,
    },
};

struct Native {
    inputs: RefCell<Vec<u64>>,
    outputs: RefCell<Vec<u64>>,
    input_bus_types: RefCell<Vec<i32>>,
    output_bus_types: RefCell<Vec<i32>>,
    input_names: RefCell<Vec<[u16; 128]>>,
    output_names: RefCell<Vec<[u16; 128]>>,
    input_flags: RefCell<Vec<u32>>,
    output_flags: RefCell<Vec<u32>>,
    input_active: RefCell<Vec<bool>>,
    output_active: RefCell<Vec<bool>>,
    reject: Cell<bool>,
    wrong_readback: Cell<bool>,
    negotiated: Cell<bool>,
    reject_activation: Cell<Option<(i32, i32)>>,
    activation: RefCell<Vec<(i32, i32, u8)>>,
}
impl Native {
    fn new(inputs: &[u64], outputs: &[u64]) -> Self {
        Self {
            inputs: RefCell::new(inputs.to_vec()),
            outputs: RefCell::new(outputs.to_vec()),
            input_bus_types: RefCell::new(default_bus_types(inputs.len())),
            output_bus_types: RefCell::new(default_bus_types(outputs.len())),
            input_names: RefCell::new(vec![[0; 128]; inputs.len()]),
            output_names: RefCell::new(vec![[0; 128]; outputs.len()]),
            input_flags: RefCell::new(vec![0; inputs.len()]),
            output_flags: RefCell::new(vec![0; outputs.len()]),
            input_active: RefCell::new(vec![false; inputs.len()]),
            output_active: RefCell::new(vec![false; outputs.len()]),
            reject: Cell::new(false),
            wrong_readback: Cell::new(false),
            negotiated: Cell::new(false),
            reject_activation: Cell::new(None),
            activation: RefCell::new(Vec::new()),
        }
    }
    fn masks(&self, direction: i32) -> &RefCell<Vec<u64>> {
        if direction == INPUT {
            &self.inputs
        } else {
            &self.outputs
        }
    }

    fn bus_types(&self, direction: i32) -> &RefCell<Vec<i32>> {
        if direction == INPUT {
            &self.input_bus_types
        } else {
            &self.output_bus_types
        }
    }

    fn names(&self, direction: i32) -> &RefCell<Vec<[u16; 128]>> {
        if direction == INPUT {
            &self.input_names
        } else {
            &self.output_names
        }
    }

    fn flags(&self, direction: i32) -> &RefCell<Vec<u32>> {
        if direction == INPUT {
            &self.input_flags
        } else {
            &self.output_flags
        }
    }

    fn active(&self, direction: i32) -> &RefCell<Vec<bool>> {
        if direction == INPUT {
            &self.input_active
        } else {
            &self.output_active
        }
    }

    fn set_default_active(&self, direction: i32, active: &[bool]) {
        *self.flags(direction).borrow_mut() = active
            .iter()
            .map(|enabled| {
                if *enabled {
                    flag_bits(BusFlags_::kDefaultActive)
                } else {
                    0
                }
            })
            .collect();
    }
}

fn default_bus_types(count: usize) -> Vec<i32> {
    (0..count)
        .map(|index| {
            if index == 0 {
                bus_type_value(BusTypes_::kMain)
            } else {
                bus_type_value(BusTypes_::kAux)
            }
        })
        .collect()
}
impl Class for Native {
    type Interfaces = (IComponent, IAudioProcessor);
}
impl IPluginBaseTrait for Native {
    unsafe fn initialize(&self, _: *mut FUnknown) -> i32 {
        kResultOk
    }
    unsafe fn terminate(&self) -> i32 {
        kResultOk
    }
}
impl IComponentTrait for Native {
    unsafe fn getControllerClassId(&self, _: *mut TUID) -> i32 {
        kInvalidArgument
    }
    unsafe fn setIoMode(&self, _: i32) -> i32 {
        kResultOk
    }
    unsafe fn getBusCount(&self, _: i32, direction: i32) -> i32 {
        i32::try_from(self.masks(direction).borrow().len()).unwrap()
    }
    unsafe fn getBusInfo(&self, _: i32, direction: i32, index: i32, info: *mut BusInfo) -> i32 {
        let Ok(index) = usize::try_from(index) else {
            return kInvalidArgument;
        };
        let masks = self.masks(direction).borrow();
        let bus_types = self.bus_types(direction).borrow();
        let names = self.names(direction).borrow();
        let flags = self.flags(direction).borrow();
        let Some(mask) = masks.get(index) else {
            return kInvalidArgument;
        };
        let Some(bus_type) = bus_types.get(index) else {
            return kInvalidArgument;
        };
        let Some(name) = names.get(index) else {
            return kInvalidArgument;
        };
        unsafe {
            *info = BusInfo {
                mediaType: AUDIO,
                direction,
                channelCount: i32::try_from(mask.count_ones()).unwrap(),
                name: *name,
                busType: *bus_type,
                flags: flags[index],
            };
        }
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _: *mut RoutingInfo, _: *mut RoutingInfo) -> i32 {
        kInvalidArgument
    }
    unsafe fn activateBus(&self, _: i32, direction: i32, index: i32, state: u8) -> i32 {
        self.activation.borrow_mut().push((direction, index, state));
        let native_index = index;
        let Ok(index) = usize::try_from(native_index) else {
            return kInvalidArgument;
        };
        let mut bus_states = self.active(direction).borrow_mut();
        let Some(active) = bus_states.get_mut(index) else {
            return kInvalidArgument;
        };
        *active = state != 0;
        if state != 0 && self.reject_activation.get() == Some((direction, native_index)) {
            kInvalidArgument
        } else {
            kResultOk
        }
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
impl IAudioProcessorTrait for Native {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut u64,
        ins: i32,
        outputs: *mut u64,
        outs: i32,
    ) -> i32 {
        if self.reject.get() {
            return kInvalidArgument;
        }
        for (pointer, count, direction) in [(inputs, ins, INPUT), (outputs, outs, OUTPUT)] {
            let Ok(count) = usize::try_from(count) else {
                return kInvalidArgument;
            };
            if count > 0 && pointer.is_null() {
                return kInvalidArgument;
            }
            *self.masks(direction).borrow_mut() = if count == 0 {
                vec![]
            } else {
                unsafe { std::slice::from_raw_parts(pointer, count) }.to_vec()
            };
        }
        self.negotiated.set(true);
        kResultOk
    }
    unsafe fn getBusArrangement(&self, direction: i32, index: i32, mask: *mut u64) -> i32 {
        let Ok(index) = usize::try_from(index) else {
            return kInvalidArgument;
        };
        let masks = self.masks(direction).borrow();
        let Some(value) = masks.get(index) else {
            return kInvalidArgument;
        };
        unsafe {
            *mask = *value ^ u64::from(self.wrong_readback.get() && self.negotiated.get());
        }
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
        let data = unsafe { &mut *data };
        let inputs = self.inputs.borrow();
        let outputs = self.outputs.borrow();
        let input_active = self.input_active.borrow();
        let output_active = self.output_active.borrow();
        let input_prefix = input_active.iter().take_while(|enabled| **enabled).count();
        let output_prefix = output_active.iter().take_while(|enabled| **enabled).count();
        if usize::try_from(data.numInputs).ok() != Some(input_prefix)
            || usize::try_from(data.numOutputs).ok() != Some(output_prefix)
        {
            return kInvalidArgument;
        }
        for (index, expected) in inputs.iter().take(input_prefix).enumerate() {
            let input = unsafe { &*data.inputs.add(index) };
            if u32::try_from(input.numChannels).ok() != Some(expected.count_ones()) {
                return kInvalidArgument;
            }
        }
        for (index, expected) in outputs.iter().take(output_prefix).enumerate() {
            let output = unsafe { &mut *data.outputs.add(index) };
            if u32::try_from(output.numChannels).ok() != Some(expected.count_ones()) {
                return kInvalidArgument;
            }
            for channel in 0..usize::try_from(output.numChannels).unwrap() {
                let source = inputs
                    .get(index)
                    .filter(|mask| index < input_prefix && channel < mask.count_ones() as usize)
                    .map(|_| unsafe {
                        *(*data.inputs.add(index))
                            .__field0
                            .channelBuffers32
                            .add(channel)
                    });
                let destination = unsafe { *output.__field0.channelBuffers32.add(channel) };
                for frame in 0..usize::try_from(data.numSamples).unwrap() {
                    unsafe {
                        *destination.add(frame) = source.map_or(0.0, |source| *source.add(frame));
                    }
                }
            }
        }
        kResultOk
    }
}

fn ranges(buses: &[Bus]) -> Vec<BusRange> {
    let mut offset = 0;
    buses
        .iter()
        .map(|bus| {
            let count = bus.channels.count() as usize;
            let range = BusRange::new(offset, count);
            offset += count;
            range
        })
        .collect()
}

fn process(
    processor: &ComPtr<IAudioProcessor>,
    buffer: &mut AudioBuffer<'_, f32>,
    layout: &NativeLayout,
) -> Result<i32> {
    with_buffers(buffer, layout, |inputs, outputs| {
        let mut data = ProcessData {
            processMode: 0,
            symbolicSampleSize: 0,
            numSamples: 2,
            numInputs: i32::try_from(inputs.len()).unwrap(),
            numOutputs: i32::try_from(outputs.len()).unwrap(),
            inputs: inputs.as_mut_ptr(),
            outputs: outputs.as_mut_ptr(),
            inputParameterChanges: ptr::null_mut(),
            outputParameterChanges: ptr::null_mut(),
            inputEvents: ptr::null_mut(),
            outputEvents: ptr::null_mut(),
            processContext: ptr::null_mut(),
        };
        unsafe { processor.process(&raw mut data) }
    })
}

#[test]
fn actual_mono_stereo_discrete_seven_and_sixty_four_arrangements_reach_all_pcm_channels() {
    for mask in [SpeakerArr::kMono, SpeakerArr::kStereo, 0x7f, u64::MAX] {
        let native = ComWrapper::new(Native::new(&[mask], &[mask]));
        let component = native.to_com_ptr::<IComponent>().unwrap();
        let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
        let layouts = discover(&component, &processor).unwrap();
        let layout = &layouts[0];
        assert_eq!(layout.layout.total_input_channels(), mask.count_ones());
        negotiate(&component, &processor, layout).unwrap();
        activate_buses(&component, layout, true).unwrap();
        let channels = mask.count_ones() as usize;
        let input = vec![[0.375_f32, -0.625]; channels];
        let mut output = vec![[0.0_f32; 2]; channels];
        let input_refs: Vec<_> = input.iter().map(<[f32; 2]>::as_slice).collect();
        let mut output_refs: Vec<_> = output.iter_mut().map(<[f32; 2]>::as_mut_slice).collect();
        let bus_range = [BusRange::new(0, channels)];
        let mut buffer = AudioBuffer::new(&input_refs, &mut output_refs, 2, &bus_range, &bus_range);
        assert_eq!(process(&processor, &mut buffer, layout).unwrap(), kResultOk);
        assert!(
            input
                .iter()
                .zip(output.iter())
                .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits))
        );
        activate_buses(&component, layout, false).unwrap();
    }
}

#[test]
fn inactive_auxiliary_bus_does_not_exclude_a_sixty_four_channel_default_layout() {
    for direction in [INPUT, OUTPUT] {
        let main = [u64::MAX];
        let with_auxiliary = [u64::MAX, SpeakerArr::kMono];
        let (inputs, outputs): (&[u64], &[u64]) = if direction == INPUT {
            (&with_auxiliary, &main)
        } else {
            (&main, &with_auxiliary)
        };
        let native = ComWrapper::new(Native::new(inputs, outputs));
        native.set_default_active(INPUT, &vec![true; inputs.len()]);
        native.set_default_active(OUTPUT, &vec![true; outputs.len()]);
        native.set_default_active(direction, &[true, false]);
        let component = native.to_com_ptr::<IComponent>().unwrap();
        let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

        let layouts = discover(&component, &processor).unwrap();
        assert_eq!(layouts.len(), 1);
        let layout = &layouts[0];
        assert_eq!(layout.layout.total_input_channels(), 64);
        assert_eq!(layout.layout.total_output_channels(), 64);
        assert_eq!(layout.input_facts.len(), inputs.len());
        assert_eq!(layout.output_facts.len(), outputs.len());
        assert!(!native.activation.borrow().contains(&(direction, 1, 1)));
        negotiate(&component, &processor, layout).unwrap();
        activate_buses(&component, layout, true).unwrap();
        let input: Vec<_> = (0_u8..64)
            .map(|index| [f32::from(index) / 64.0, -0.625])
            .collect();
        let mut output = [[0.0_f32; 2]; 64];
        let input_refs: Vec<_> = input.iter().map(<[f32; 2]>::as_slice).collect();
        let mut output_refs: Vec<_> = output.iter_mut().map(<[f32; 2]>::as_mut_slice).collect();
        let ranges = [BusRange::new(0, 64)];
        let mut buffer = AudioBuffer::new(&input_refs, &mut output_refs, 2, &ranges, &ranges);
        assert_eq!(process(&processor, &mut buffer, layout).unwrap(), kResultOk);
        assert!(
            input
                .iter()
                .zip(output)
                .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits))
        );
        activate_buses(&component, layout, false).unwrap();
        assert!(native.input_active.borrow().iter().all(|active| !active));
        assert!(native.output_active.borrow().iter().all(|active| !active));
    }
}

#[test]
fn oversized_active_layouts_are_rejected_before_native_negotiation_or_activation() {
    for direction in [INPUT, OUTPUT] {
        let native = ComWrapper::new(Native::new(
            &[u64::MAX, SpeakerArr::kMono],
            &[u64::MAX, SpeakerArr::kMono],
        ));
        native.set_default_active(INPUT, &[true, false]);
        native.set_default_active(OUTPUT, &[true, false]);
        native.set_default_active(direction, &[true, true]);
        let component = native.to_com_ptr::<IComponent>().unwrap();
        let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

        assert!(discover(&component, &processor).is_err());
        assert!(native.activation.borrow().is_empty());
        assert!(!native.negotiated.get());
        let topology = read_topology(&component, &processor).unwrap();
        let oversized = make_layout(&topology, 2, 2).unwrap();
        assert!(negotiate(&component, &processor, &oversized).is_err());
        assert!(!native.negotiated.get());
    }
}

#[test]
fn sidechain_auxiliary_and_asymmetric_buses_are_not_silently_dropped() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
    ));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layouts = discover(&component, &processor).unwrap();
    let layout = &layouts[0];
    assert_eq!(layout.layout.inputs[1].kind, BusKind::Sidechain);
    assert_eq!(layout.layout.outputs[1].kind, BusKind::Auxiliary);
    negotiate(&component, &processor, layout).unwrap();
    activate_buses(&component, layout, true).unwrap();
    let input = [[0.125_f32; 2], [0.25; 2], [0.75; 2]];
    let mut output = [[0.0_f32; 2]; 3];
    let refs: Vec<_> = input.iter().map(<[f32; 2]>::as_slice).collect();
    let mut out_refs: Vec<_> = output.iter_mut().map(<[f32; 2]>::as_mut_slice).collect();
    let ins = ranges(&layout.layout.inputs);
    let outs = ranges(&layout.layout.outputs);
    let mut buffer = AudioBuffer::new(&refs, &mut out_refs, 2, &ins, &outs);
    assert_eq!(process(&processor, &mut buffer, layout).unwrap(), kResultOk);
    assert_eq!(
        input.map(|v| v.map(f32::to_bits)),
        output.map(|v| v.map(f32::to_bits))
    );
    activate_buses(&component, layout, false).unwrap();
}

#[test]
fn exact_native_masks_and_contiguous_default_prefixes_are_advertised_with_distinct_ids() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
    ));
    native.set_default_active(INPUT, &[true, false]);
    native.set_default_active(OUTPUT, &[true, false]);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layouts = discover(&component, &processor).unwrap();

    assert_eq!(layouts.len(), 4);
    assert_eq!(layouts[0].layout.inputs.len(), 2);
    assert_eq!(layouts[0].layout.outputs.len(), 2);
    assert_eq!(layouts[1].layout.inputs.len(), 1);
    assert_eq!(layouts[1].layout.outputs.len(), 1);
    assert_ne!(layouts[0].layout.id, layouts[1].layout.id);
    assert!(layouts.iter().all(|layout| {
        layout.layout.id.as_ref().is_some_and(|id| id.len() == 72)
            && layout
                .input_facts
                .iter()
                .map(|fact| fact.arrangement)
                .eq([SpeakerArr::kStereo, SpeakerArr::kMono])
            && layout
                .output_facts
                .iter()
                .map(|fact| fact.arrangement)
                .eq([SpeakerArr::kStereo, SpeakerArr::kMono])
    }));
    assert!(select_layout(&layouts, &layouts[1].layout).is_ok());
    let mut forged = layouts[1].layout.clone();
    forged.id = layouts[0].layout.id.clone();
    assert!(select_layout(&layouts, &forged).is_err());

    let prefix = &layouts[1];
    negotiate(&component, &processor, prefix).unwrap();
    activate_buses(&component, prefix, true).unwrap();
    assert_eq!(native.input_active.borrow().as_slice(), &[true, false]);
    assert_eq!(native.output_active.borrow().as_slice(), &[true, false]);

    let input = [[0.125_f32; 2], [-0.25; 2]];
    let mut output = [[0.0_f32; 2]; 2];
    let input_refs: Vec<_> = input.iter().map(<[f32; 2]>::as_slice).collect();
    let mut output_refs: Vec<_> = output.iter_mut().map(<[f32; 2]>::as_mut_slice).collect();
    let ranges = [BusRange::new(0, 2)];
    let mut buffer = AudioBuffer::new(&input_refs, &mut output_refs, 2, &ranges, &ranges);
    assert_eq!(process(&processor, &mut buffer, prefix).unwrap(), kResultOk);
    assert_eq!(
        input.map(|channel| channel.map(f32::to_bits)),
        output.map(|channel| channel.map(f32::to_bits))
    );
    activate_buses(&component, prefix, false).unwrap();
    assert!(native.input_active.borrow().iter().all(|active| !active));
    assert!(native.output_active.borrow().iter().all(|active| !active));
}

#[test]
fn optional_bus_prefix_combinations_are_probed_and_deliver_only_active_pcm() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono, SpeakerArr::kMono],
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
    ));
    native.set_default_active(INPUT, &[true, false, false]);
    native.set_default_active(OUTPUT, &[true, false]);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layouts = discover(&component, &processor).unwrap();
    let combinations: std::collections::BTreeSet<_> = layouts
        .iter()
        .map(|layout| (layout.input_active_prefix, layout.output_active_prefix))
        .collect();
    assert_eq!(
        combinations,
        [(1, 1), (1, 2), (2, 1), (2, 2), (3, 1), (3, 2)].into()
    );
    let middle = layouts
        .iter()
        .find(|layout| layout.input_active_prefix == 2 && layout.output_active_prefix == 1)
        .unwrap();
    negotiate(&component, &processor, middle).unwrap();
    activate_buses(&component, middle, true).unwrap();
    assert_eq!(*native.input_active.borrow(), [true, true, false]);
    assert_eq!(*native.output_active.borrow(), [true, false]);
    let inputs = [[0.125_f32; 2], [-0.25; 2], [0.5; 2]];
    let mut outputs = [[0.0_f32; 2]; 2];
    let input_refs: Vec<_> = inputs.iter().map(<[f32; 2]>::as_slice).collect();
    let mut output_refs: Vec<_> = outputs.iter_mut().map(<[f32; 2]>::as_mut_slice).collect();
    let input_ranges = ranges(&middle.layout.inputs);
    let output_ranges = ranges(&middle.layout.outputs);
    let mut buffer = AudioBuffer::new(
        &input_refs,
        &mut output_refs,
        2,
        &input_ranges,
        &output_ranges,
    );
    assert_eq!(process(&processor, &mut buffer, middle).unwrap(), kResultOk);
    assert_eq!(
        outputs.map(|channel| channel.map(f32::to_bits)),
        [inputs[0], inputs[1]].map(|channel| channel.map(f32::to_bits))
    );
    activate_buses(&component, middle, false).unwrap();
    assert!(native.input_active.borrow().iter().all(|active| !active));
    assert!(native.output_active.borrow().iter().all(|active| !active));
}

#[test]
fn optional_output_does_not_read_a_deactivated_input_bus() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
    ));
    native.set_default_active(INPUT, &[true, false]);
    native.set_default_active(OUTPUT, &[true, false]);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layouts = discover(&component, &processor).unwrap();
    let layout = layouts
        .iter()
        .find(|layout| layout.input_active_prefix == 1 && layout.output_active_prefix == 2)
        .unwrap();
    negotiate(&component, &processor, layout).unwrap();
    activate_buses(&component, layout, true).unwrap();
    let inputs = [[0.125_f32; 2], [-0.25; 2]];
    let mut outputs = [[1.0_f32; 2]; 3];
    let input_refs: Vec<_> = inputs.iter().map(<[f32; 2]>::as_slice).collect();
    let mut output_refs: Vec<_> = outputs.iter_mut().map(<[f32; 2]>::as_mut_slice).collect();
    let input_ranges = ranges(&layout.layout.inputs);
    let output_ranges = ranges(&layout.layout.outputs);
    let mut buffer = AudioBuffer::new(
        &input_refs,
        &mut output_refs,
        2,
        &input_ranges,
        &output_ranges,
    );
    assert_eq!(process(&processor, &mut buffer, layout).unwrap(), kResultOk);
    assert_eq!(
        outputs.map(|channel| channel.map(f32::to_bits)),
        [inputs[0], inputs[1], [0.0; 2]].map(|channel| channel.map(f32::to_bits))
    );
    activate_buses(&component, layout, false).unwrap();
    assert!(native.input_active.borrow().iter().all(|active| !active));
    assert!(native.output_active.borrow().iter().all(|active| !active));
}

#[test]
fn zero_default_outputs_advertise_nonzero_prefixes_and_process_pcm() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo],
        &[SpeakerArr::kStereo; 3],
    ));
    native.set_default_active(INPUT, &[true]);
    native.set_default_active(OUTPUT, &[false; 3]);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

    let layouts = discover(&component, &processor).unwrap();
    assert_eq!(
        layouts
            .iter()
            .map(|layout| layout.output_active_prefix)
            .collect::<Vec<_>>(),
        [3, 0, 1, 2]
    );
    assert!(native.output_active.borrow().iter().all(|active| !active));
    let middle = &layouts[3];
    negotiate(&component, &processor, middle).unwrap();
    activate_buses(&component, middle, true).unwrap();
    assert_eq!(*native.output_active.borrow(), [true, true, false]);

    let input = [[0.125_f32, -0.25], [0.5, -0.75]];
    let mut output = [[1.0_f32; 2]; 4];
    let input_refs: Vec<_> = input.iter().map(<[f32; 2]>::as_slice).collect();
    let mut output_refs: Vec<_> = output.iter_mut().map(<[f32; 2]>::as_mut_slice).collect();
    let input_ranges = ranges(&middle.layout.inputs);
    let output_ranges = ranges(&middle.layout.outputs);
    let mut buffer = AudioBuffer::new(
        &input_refs,
        &mut output_refs,
        2,
        &input_ranges,
        &output_ranges,
    );
    assert_eq!(process(&processor, &mut buffer, middle).unwrap(), kResultOk);
    assert_eq!(output, [input[0], input[1], [0.0; 2], [0.0; 2]]);
    activate_buses(&component, middle, false).unwrap();
    assert!(native.input_active.borrow().iter().all(|active| !active));
    assert!(native.output_active.borrow().iter().all(|active| !active));
}

#[test]
fn zero_default_outputs_rejected_full_probe_rolls_back_and_keeps_intermediates() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo],
        &[SpeakerArr::kStereo; 3],
    ));
    native.set_default_active(INPUT, &[true]);
    native.set_default_active(OUTPUT, &[false; 3]);
    native.reject_activation.set(Some((OUTPUT, 2)));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

    let layouts = discover(&component, &processor).unwrap();
    assert_eq!(
        layouts
            .iter()
            .map(|layout| layout.output_active_prefix)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert!(native.activation.borrow().contains(&(OUTPUT, 2, 1)));
    assert!(native.activation.borrow().contains(&(OUTPUT, 2, 0)));
    assert!(native.input_active.borrow().iter().all(|active| !active));
    assert!(native.output_active.borrow().iter().all(|active| !active));
}

#[test]
fn rejected_optional_activation_is_omitted_and_discovery_releases_all_buses() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono, SpeakerArr::kMono],
        &[SpeakerArr::kStereo],
    ));
    native.set_default_active(INPUT, &[true, false, false]);
    native.set_default_active(OUTPUT, &[true]);
    native.reject_activation.set(Some((INPUT, 2)));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

    let layouts = discover(&component, &processor).unwrap();
    assert_eq!(layouts.len(), 2);
    assert_eq!(layouts[0].input_active_prefix, 1);
    assert_eq!(layouts[1].input_active_prefix, 2);
    assert!(native.input_active.borrow().iter().all(|active| !active));
    assert!(native.output_active.borrow().iter().all(|active| !active));
}

#[test]
fn native_layout_identity_is_stable_and_binds_exact_arrangement_masks() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, 0x5],
        &[SpeakerArr::kStereo],
    ));
    native.set_default_active(INPUT, &[true, false]);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let first = discover(&component, &processor).unwrap();
    let second = discover(&component, &processor).unwrap();
    assert_eq!(first[0].layout.id, second[0].layout.id);

    let changed_mask = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, 0x9],
        &[SpeakerArr::kStereo],
    ));
    changed_mask.set_default_active(INPUT, &[true, false]);
    let changed_component = changed_mask.to_com_ptr::<IComponent>().unwrap();
    let changed_processor = changed_mask.to_com_ptr::<IAudioProcessor>().unwrap();
    let changed = discover(&changed_component, &changed_processor).unwrap();
    assert_ne!(first[0].layout.id, changed[0].layout.id);
    assert_eq!(
        first[0].layout.inputs[1].channels,
        changed[0].layout.inputs[1].channels
    );
}

#[test]
fn interior_default_active_hole_does_not_create_a_guessed_prefix_layout() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono, SpeakerArr::kMono],
        &[SpeakerArr::kStereo],
    ));
    native.set_default_active(INPUT, &[true, false, true]);
    native.set_default_active(OUTPUT, &[true]);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

    let layouts = discover(&component, &processor).unwrap();
    assert_eq!(layouts.len(), 1);
    assert_eq!(layouts[0].layout.inputs.len(), 3);
    assert_eq!(layouts[0].layout.outputs.len(), 1);
}

#[test]
fn control_voltage_buses_are_rejected_instead_of_routed_as_audio() {
    let native = ComWrapper::new(Native::new(&[SpeakerArr::kMono], &[SpeakerArr::kMono]));
    native.input_flags.borrow_mut()[0] = flag_bits(BusFlags_::kIsControlVoltage);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

    let error = discover(&component, &processor).expect_err("CV has no audio route type");
    assert!(
        error
            .to_string()
            .contains("unsupported control-voltage bus")
    );
}

#[test]
fn failed_baseline_restoration_fails_discovery() {
    let native = ComWrapper::new(Native::new(&[SpeakerArr::kStereo], &[SpeakerArr::kStereo]));
    native.reject.set(true);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();

    assert!(discover(&component, &processor).is_err());
}

#[test]
fn negotiation_rejection_readback_mismatch_and_activation_failure_are_errors() {
    let native = ComWrapper::new(Native::new(&[SpeakerArr::kStereo], &[SpeakerArr::kStereo]));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layouts = discover(&component, &processor).unwrap();
    let layout = &layouts[0];
    native.reject.set(true);
    assert!(negotiate(&component, &processor, layout).is_err());
    native.reject.set(false);
    native.wrong_readback.set(true);
    assert!(negotiate(&component, &processor, layout).is_err());
    native.wrong_readback.set(false);
    native.reject_activation.set(Some((OUTPUT, 0)));
    assert!(activate_buses(&component, layout, true).is_err());
    assert!(native.activation.borrow().contains(&(INPUT, 0, 0)));
    assert!(native.activation.borrow().contains(&(OUTPUT, 0, 0)));
    let mut invalid = layout.clone();
    invalid.layout.inputs[0].channels = ChannelConfig::Discrete(65);
    assert!(negotiate(&component, &processor, &invalid).is_err());
}

#[test]
fn negotiation_rejects_same_width_bus_role_or_control_voltage_drift() {
    let role_drift = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
        &[SpeakerArr::kStereo],
    ));
    let component = role_drift.to_com_ptr::<IComponent>().unwrap();
    let processor = role_drift.to_com_ptr::<IAudioProcessor>().unwrap();
    let layout = discover(&component, &processor).unwrap().remove(0);
    role_drift.input_bus_types.borrow_mut()[1] = bus_type_value(BusTypes_::kMain);
    assert!(negotiate(&component, &processor, &layout).is_err());

    let cv_drift = ComWrapper::new(Native::new(&[SpeakerArr::kMono], &[SpeakerArr::kMono]));
    let component = cv_drift.to_com_ptr::<IComponent>().unwrap();
    let processor = cv_drift.to_com_ptr::<IAudioProcessor>().unwrap();
    let layout = discover(&component, &processor).unwrap().remove(0);
    cv_drift.input_flags.borrow_mut()[0] = flag_bits(BusFlags_::kIsControlVoltage);
    assert!(negotiate(&component, &processor, &layout).is_err());
}

#[test]
fn negotiation_checks_native_facts_for_excluded_trailing_buses() {
    let native = ComWrapper::new(Native::new(
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
        &[SpeakerArr::kStereo, SpeakerArr::kMono],
    ));
    native.set_default_active(INPUT, &[true, false]);
    native.set_default_active(OUTPUT, &[true, false]);
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layouts = discover(&component, &processor).unwrap();
    let prefix = &layouts[1];
    assert_eq!(prefix.input_active_prefix, 1);
    assert_eq!(prefix.output_active_prefix, 1);

    native.output_bus_types.borrow_mut()[1] = bus_type_value(BusTypes_::kMain);
    assert!(negotiate(&component, &processor, prefix).is_err());
}

#[test]
fn native_bus_name_changes_do_not_invalidate_selected_topology() {
    let native = ComWrapper::new(Native::new(&[SpeakerArr::kMono], &[SpeakerArr::kMono]));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layout = discover(&component, &processor).unwrap().remove(0);
    native.input_names.borrow_mut()[0][0] = u16::from(b'N');

    assert!(negotiate(&component, &processor, &layout).is_ok());
}

#[test]
fn mismatched_process_geometry_is_rejected_before_the_native_callback() {
    let native = ComWrapper::new(Native::new(&[SpeakerArr::kStereo], &[SpeakerArr::kStereo]));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layout = discover(&component, &processor).unwrap().remove(0);
    let input: [&[f32]; 1] = [&[0.0_f32; 2]];
    let mut output = [0.0_f32; 2];
    let mut outputs: [&mut [f32]; 1] = [&mut output];
    let ranges = [BusRange::new(0, 1)];
    let mut buffer = AudioBuffer::new(&input, &mut outputs, 2, &ranges, &ranges);
    let heap = crate::test_support::count_heap_operations(|| {
        assert!(matches!(
            with_buffers(&mut buffer, &layout, |_, _| panic!(
                "must not call native processor"
            )),
            Err(Error::Process {
                fault: truce_rack_core::error::ProcessFault::InvalidAudioMapping,
                ..
            })
        ));
    });
    assert_eq!(heap, (0, 0));
}

#[test]
fn stack_bus_binding_removes_twenty_thousand_allocations_over_ten_thousand_stereo_blocks() {
    let native = ComWrapper::new(Native::new(&[SpeakerArr::kStereo], &[SpeakerArr::kStereo]));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layout = discover(&component, &processor).unwrap().remove(0);
    let input: [&[f32]; 2] = [&[0.0_f32; 2], &[0.0_f32; 2]];
    let mut left = [0.0_f32; 2];
    let mut right = [0.0_f32; 2];
    let mut output: [&mut [f32]; 2] = [&mut left, &mut right];
    let ranges = [BusRange::new(0, 2)];
    let mut buffer = AudioBuffer::new(&input, &mut output, 2, &ranges, &ranges);
    let before = crate::parameter_delivery::count_allocations(|| {
        for _ in 0..10_000 {
            let ins: Vec<*mut f32> = buffer
                .main_inputs()
                .iter()
                .map(|channel| channel.as_ptr().cast_mut())
                .collect();
            let outs: Vec<*mut f32> = buffer
                .main_outputs()
                .iter_mut()
                .map(|channel| channel.as_mut_ptr())
                .collect();
            std::hint::black_box((ins, outs));
        }
    });
    let after = crate::parameter_delivery::count_allocations(|| {
        for _ in 0..10_000 {
            with_buffers(&mut buffer, &layout, |inputs, outputs| {
                std::hint::black_box((inputs, outputs));
            })
            .unwrap();
        }
    });
    assert_eq!(before, 20_000);
    assert_eq!(after, 0);
}

#[test]
fn invalid_bus_spans_fail_before_native_call_without_heap_use() {
    let native = ComWrapper::new(Native::new(&[SpeakerArr::kStereo], &[SpeakerArr::kStereo]));
    let component = native.to_com_ptr::<IComponent>().unwrap();
    let processor = native.to_com_ptr::<IAudioProcessor>().unwrap();
    let layout = discover(&component, &processor).unwrap().remove(0);
    let input: [&[f32]; 2] = [&[0.25; 2], &[-0.5; 2]];
    let good = BusRange::new(0, 2);
    for invalid in [BusRange::new(usize::MAX, 2), BusRange::new(1, 2)] {
        for (input_range, output_range) in [(invalid, good), (good, invalid)] {
            let mut left = [0.75_f32; 2];
            let mut right = [0.75_f32; 2];
            let mut output: [&mut [f32]; 2] = [&mut left, &mut right];
            let inputs = [input_range];
            let outputs = [output_range];
            let mut buffer = AudioBuffer::new(&input, &mut output, 2, &inputs, &outputs);
            let heap = crate::test_support::count_heap_operations(|| {
                assert!(matches!(
                    with_buffers(&mut buffer, &layout, |_, _| panic!(
                        "invalid span reached native DSP"
                    )),
                    Err(Error::Process {
                        fault: truce_rack_core::error::ProcessFault::InvalidAudioMapping,
                        ..
                    })
                ));
            });
            assert_eq!(heap, (0, 0));
            assert_eq!(left.map(f32::to_bits), [0.75_f32.to_bits(); 2]);
            assert_eq!(right.map(f32::to_bits), [0.75_f32.to_bits(); 2]);
        }
    }
}
