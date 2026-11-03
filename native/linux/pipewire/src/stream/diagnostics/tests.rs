use super::super::NativeCounters;
use super::*;

#[test]
fn transfer_snapshot_abi_and_all_stage_words_match_c_layout() {
    assert_eq!(std::mem::size_of::<NativeCounters>(), 16);
    assert_eq!(std::mem::size_of::<RejectedTransfer>(), 48);
    assert_eq!(std::mem::align_of::<RejectedTransfer>(), 8);
    assert_eq!(std::mem::offset_of!(RejectedTransfer, first_clock), 32);
    assert_eq!(std::mem::size_of::<NativeTransferSnapshot>(), 256);
    assert_eq!(std::mem::align_of::<NativeTransferSnapshot>(), 8);
    assert_eq!(std::mem::offset_of!(NativeTransferSnapshot, failures), 24);
    assert_eq!(
        std::mem::offset_of!(NativeTransferSnapshot, engine_read_frames),
        216
    );
    assert_eq!(
        std::mem::offset_of!(NativeTransferSnapshot, native_restart_calls),
        240
    );
    assert_eq!(
        std::mem::offset_of!(NativeTransferSnapshot, native_output_trigger_rejections),
        248
    );
    let words = std::array::from_fn(|index| index as u64 + 1);
    let snapshot = TransferSnapshot::from_words(&words);
    assert_eq!(snapshot.words(), words);
    assert_eq!(snapshot.native_clock, words[29]);
    assert_eq!(snapshot.native_quantum, words[30]);
    assert_eq!(snapshot.counters.engine_read_frames, words[31]);
    assert_eq!(snapshot.counters.consumer_discard_calls, words[32]);
    assert_eq!(snapshot.counters.consumer_discard_frames, words[33]);
    assert_eq!(snapshot.counters.native_restart_calls, words[34]);
    assert_eq!(
        snapshot.counters.native_output_trigger_rejections,
        words[35]
    );
    for (stage, failure) in snapshot.counters.failures.iter().enumerate() {
        assert_eq!(failure.rejected_calls, words[4 + stage * 6]);
        assert_eq!(failure.last_clock, words[9 + stage * 6]);
    }
}

#[test]
fn transfer_debug_labels_read_and_write_failures_without_changing_words() {
    let words = std::array::from_fn(|index| index as u64 + 1);
    let snapshot = TransferSnapshot::from_words(&words);
    let diagnostic = format!("{:?}", snapshot.counters);
    for (stage, label) in [
        "native_capture_write",
        "native_output_read",
        "engine_capture_read",
        "engine_output_write",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(diagnostic.contains(&format!(
            "{label}: RejectedTransfer {{ rejected_calls: {}",
            words[4 + stage * 6]
        )));
    }
    assert!(!diagnostic.contains("failures: ["));
    assert_eq!(snapshot.words(), words);
}

#[test]
fn transfer_totals_wrap_without_joining_multiple_stream_clocks() {
    let first = NativeTransferSnapshot {
        read: u64::MAX,
        engine_read_frames: u64::MAX,
        consumer_discard_calls: u64::MAX,
        consumer_discard_frames: u64::MAX,
        native_restart_calls: u64::MAX,
        native_output_trigger_rejections: u64::MAX,
        failures: [RejectedTransfer {
            rejected_calls: u64::MAX,
            rejected_frames: u64::MAX,
            first_clock: 7_u64 << 32 | 0xbb80,
            last_clock: 7_u64 << 32 | 0xbb80,
            first_position: 128,
            last_position: 256,
        }; 4],
        ..NativeTransferSnapshot::default()
    };
    let mut snapshot = TransferSnapshot::default();
    assert_eq!(snapshot.words(), [0; TRANSFER_SNAPSHOT_WORDS]);
    snapshot.include_counters(first);
    assert_eq!(snapshot.streams, 1);
    snapshot.include_counters(NativeTransferSnapshot {
        read: 2,
        engine_read_frames: 2,
        consumer_discard_calls: 2,
        consumer_discard_frames: 2,
        native_restart_calls: 2,
        native_output_trigger_rejections: 2,
        failures: [RejectedTransfer {
            rejected_calls: 2,
            rejected_frames: 4,
            first_clock: 8_u64 << 32 | 0xbb80,
            last_clock: 8_u64 << 32 | 0xbb80,
            first_position: 1,
            last_position: 10,
        }; 4],
        ..NativeTransferSnapshot::default()
    });
    assert_eq!(snapshot.streams, 2);
    assert_eq!(snapshot.counters.read, 1);
    assert_eq!(snapshot.counters.engine_read_frames, 1);
    assert_eq!(snapshot.counters.consumer_discard_calls, 1);
    assert_eq!(snapshot.counters.consumer_discard_frames, 1);
    assert_eq!(snapshot.counters.native_restart_calls, 1);
    assert_eq!(snapshot.counters.native_output_trigger_rejections, 1);
    for failure in snapshot.counters.failures {
        assert_eq!(failure.rejected_calls, 1);
        assert_eq!(failure.rejected_frames, 3);
        assert_eq!(failure.first_clock, first.failures[0].first_clock);
        assert_eq!(failure.last_position, 256);
    }
    assert_eq!(1_u64.wrapping_sub(u64::MAX), 2);
    assert_eq!(
        TransferSnapshot::default().words(),
        [0; TRANSFER_SNAPSHOT_WORDS]
    );
}
