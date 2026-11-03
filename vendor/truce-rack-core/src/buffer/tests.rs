//! Invalid native bus metadata cannot reach unchecked channel slicing.
use super::*;

#[test]
fn checked_bus_access_rejects_invalid_extents_without_rt_allocation() {
    for range in [BusRange::new(1, 1), BusRange::new(usize::MAX, 1)] {
        let input = [0.25_f32; 4];
        let inputs = [&input[..]];
        let mut output = [0.0_f32; 4];
        let mut outputs = [&mut output[..]];
        let ranges = [range];
        let mut audio = AudioBuffer::new(&inputs, &mut outputs, 4, &ranges, &ranges);
        let measured = allocation_counter::measure(|| {
            assert!(audio.try_bus_inputs(0).is_none());
            assert!(audio.try_bus_outputs(0).is_none());
            assert!(audio.try_bus_inputs(1).is_none());
            assert!(audio.try_bus_outputs(1).is_none());
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
    }
}

#[test]
fn checked_bus_access_borrows_the_original_channel_storage() {
    let input = [0.25_f32; 4];
    let inputs = [&input[..]];
    let mut output = [0.0_f32; 4];
    let mut outputs = [&mut output[..]];
    let ranges = [BusRange::new(0, 1)];
    let mut audio = AudioBuffer::new(&inputs, &mut outputs, 4, &ranges, &ranges);
    assert!(
        audio.try_bus_inputs(0).unwrap()[0]
            .iter()
            .map(|sample| sample.to_bits())
            .eq(input.map(f32::to_bits))
    );
    audio.try_bus_outputs(0).unwrap()[0].fill(0.5);
    assert_eq!(output.map(f32::to_bits), [0.5_f32.to_bits(); 4]);
}
