use crate::{
    abi,
    io_admission::{COUNTERS, Stage},
    tests::{Fixture, OK, lock},
};

#[test]
fn rejected_sdk_coordinates_and_registry_admission_remain_visible_without_native_rows() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    assert_eq!(fixture.registry.client(id, 7, true), OK);
    assert_eq!(fixture.registry.start(id, 7, 0), OK);
    let pin = fixture.registry.pin(id).unwrap();
    pin.io_trace.set_enabled(true);
    let registry = (&raw const *fixture.registry).cast_mut();
    let mut pcm = [0.25_f32; 2];
    let pointer = pcm.as_mut_ptr().cast();
    // SAFETY: the fixture retains the registry; valid extents own one stereo frame.
    let invoke = |stream, sample, buffer, frames| unsafe {
        abi::mr_hal_core_io_sample_time(registry, id, stream, false, 7, sample, buffer, frames)
    };
    assert!(!invoke(id + 1, 10.5, pointer, 1));
    assert!(!invoke(id + 1, f64::NAN, pointer, 1));
    assert!(!invoke(id + 1, 10.0, std::ptr::null_mut(), 1));
    assert!(!invoke(id + 1, 10.0, pointer, u32::MAX));
    assert!(!invoke(id + 2, 10.0, pointer, 1));
    let control = lock(&pin.control);
    let frozen = pin.freeze();
    let allocation = allocation_counter::measure(|| {
        assert!(!invoke(id + 1, 10.0, pointer, 1));
    });
    assert_eq!(allocation.count_total, 0);
    drop(frozen);
    drop(control);
    // These early returns do not touch host PCM and used to leave no evidence.
    assert_eq!(pcm.map(f32::to_bits), [0.25_f32.to_bits(); 2]);
    let counters = pin.io_trace.admission.snapshot();
    assert_eq!(&counters[..8], &[6, 0, 2, 2, 1, 1, 0, 0]);
    assert_eq!(counters[8], f64::NAN.to_bits());
    let words: Vec<_> = pin
        .io_trace
        .snapshot()
        .chunks_exact(8)
        .map(|word| u64::from_ne_bytes(word.try_into().unwrap()))
        .collect();
    assert_eq!((words[0], words[3]), (5, 0));
    assert_eq!(&words[5..crate::io_trace::HEADER_WORDS], &counters);
    assert_eq!(counters[9], u64::from(u32::MAX) | (1 << 32));
    assert_eq!(&counters[10..], &[1, 1]);
    assert!(invoke(id + 1, 10.0, pointer, 1));
    assert_eq!(pcm.map(f32::to_bits), [0; 2]);
    assert_eq!(pin.io_trace.admission.snapshot()[0], 7);
}

#[test]
fn disabled_admission_does_not_retain_previous_measurement_counts() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let pin = fixture.registry.pin(id).unwrap();
    pin.io_trace.set_enabled(true);
    pin.io_trace.reject_timestamp(10.5, false);
    assert_eq!(pin.io_trace.admission.snapshot()[2], 1);
    pin.io_trace.set_enabled(false);
    pin.io_trace.reject_timestamp(11.5, false);
    pin.io_trace.note(Stage::Frozen);
    assert_eq!(pin.io_trace.admission.snapshot(), [0; COUNTERS + 4]);
}
