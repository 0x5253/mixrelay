//! External callback admission, transfer validation and real-time allocation checks.
use super::*;

#[test]
fn only_explicit_active_demand_counts_missing_input_over_375_cycles() {
    use mixrelay_audio_io::process::InputActivity;
    for (activity, read_frames, expected) in [
        (InputActivity::Unknown, 0, 0),
        (InputActivity::Inactive, 0, 0),
        (InputActivity::Priming, 0, 0),
        (InputActivity::Active, 0, 375),
        (InputActivity::Active, 4, 0),
    ] {
        let (mut processor, mut io, cycle) = callback_fixture();
        io.activity = activity;
        io.read_frames = read_frames;
        let mut before = 0;
        let mut after = 0;
        for index in 0..375 {
            let report = processor
                .process_external_cycle(
                    ProcessCycle {
                        first_frame: index * u64::from(cycle.frames),
                        ..cycle
                    },
                    &mut io,
                )
                .unwrap()
                .expect("fixture admits every valid cycle");
            before += u64::from(read_frames < cycle.frames);
            after += report.input_starvations;
            assert_eq!(report.output_overflows, 0);
            assert_eq!(io.output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
        }
        assert_eq!(after, expected, "activity={activity:?}");
        assert_eq!(before, if read_frames == 0 { 375 } else { 0 });
        eprintln!("activity={activity:?} frames={read_frames}: previous={before}, current={after}");
    }
}

#[test]
fn silent_callback_is_success_but_short_transfers_are_reported() {
    let (mut processor, mut io, cycle) = callback_fixture();
    assert_eq!(
        processor.process_external_cycle(cycle, &mut io).unwrap(),
        Some(ProcessReport::default())
    );
    assert_eq!(io.output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
    io.read_frames = 2;
    io.write_frames = 3;
    io.value = 1.0;
    let next = ProcessCycle {
        first_frame: 4,
        ..cycle
    };
    let report = processor
        .process_external_cycle(next, &mut io)
        .unwrap()
        .expect("fixture admits every valid cycle");
    assert_eq!(report.input_starvations, 1);
    assert_eq!(report.output_overflows, 1);
    assert_eq!(
        io.output.map(f32::to_bits),
        [1.0_f32, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0].map(f32::to_bits)
    );
}

#[test]
fn bad_clock_or_format_never_touches_native_io() {
    let (mut processor, mut io, cycle) = callback_fixture();
    for bad in [
        ProcessCycle {
            sample_rate: 44_100,
            ..cycle
        },
        ProcessCycle { frames: 8, ..cycle },
        ProcessCycle { frames: 0, ..cycle },
    ] {
        assert!(processor.process_external_cycle(bad, &mut io).is_err());
    }
    assert_eq!((io.reads, io.writes), (0, 0));
    processor.process_external_cycle(cycle, &mut io).unwrap();
    assert!(processor.process_external_cycle(cycle, &mut io).is_err());
    assert_eq!((io.reads, io.writes), (1, 1));
}

#[test]
fn oversized_transfer_is_an_adapter_contract_failure() {
    let (mut processor, mut io, cycle) = callback_fixture();
    io.read_frames = 5;
    assert_eq!(
        processor.process_external_cycle(cycle, &mut io),
        Err(ProcessBlockError::InvalidTransfer)
    );
    assert_eq!(io.writes, 0);
}

#[test]
fn validated_audio_cycles_do_not_allocate_or_free() {
    let (mut processor, mut io, mut cycle) = callback_fixture();
    HEAP_OPERATIONS.with(|count| count.set(Some(0)));
    for _ in 0..10_000 {
        processor.process_external_cycle(cycle, &mut io).unwrap();
        cycle.first_frame += u64::from(cycle.frames);
    }
    let operations = HEAP_OPERATIONS.with(|count| count.replace(None));
    assert_eq!(operations, Some(0));
}
