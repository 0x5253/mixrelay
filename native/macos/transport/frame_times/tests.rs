use super::*;
#[test]
fn ring_wrap_rejects_stale_coordinates_and_preserves_partial_ranges() {
    assert!(FrameTimes::new(0).is_none());
    let times = FrameTimes::new(4).unwrap();
    let mut output = [0; 2];
    let measured = allocation_counter::measure(|| {
        times.range(0, 4, 1000, 20.5);
        assert!(times.read(1, &mut output));
        assert_eq!(output, [1021, 1041]);
        times.publish(4, &[2000, 2021]);
        assert_eq!(times.at(0), None);
        assert_eq!(times.at(1), None);
        assert_eq!(times.at(2), Some(1041));
        assert!(times.read(4, &mut output));
        assert_eq!(output, [2000, 2021]);
        assert_eq!(times.locate(2, 6, 2005), Some(4));
        times.range(6, 2, 0, f64::NAN);
        assert_eq!(times.at(6), Some(0));
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn replacement_race_never_accepts_another_frames_coordinate() {
    let times = std::sync::Arc::new(FrameTimes::new(8).unwrap());
    let producer = times.clone();
    let thread = std::thread::spawn(move || {
        for frame in 0..100_000_u64 {
            producer.publish(frame, &[frame + 1_000_000]);
        }
    });
    for frame in 0..100_000_u64 {
        if let Some(time) = times.at(frame) {
            assert_eq!(time, frame + 1_000_000);
        }
    }
    thread.join().unwrap();
}
