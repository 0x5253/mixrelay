use super::*;

#[test]
fn selection_epoch_rollover_preserves_the_complete_control_identity() {
    let capture = LearnCapture::default();
    capture.state.store(EPOCH_MASK | WAITING, Ordering::Release);
    capture.offer_with(1, &[0x90, 60, 100], &mut |input, previous, encoded| {
        capture.arm();
        input.compare_exchange(previous, encoded, Ordering::AcqRel, Ordering::Acquire)
    });
    assert_eq!(capture.captured(), None);
    capture.offer(u32::MAX, &[0xbf, 127, 127]);
    assert_eq!(capture.captured(), Some((u32::MAX, 15, Kind::Cc, 127)));
    capture.cancel();
    assert!(!capture.active());
}

#[test]
fn concurrent_offers_and_reads_skip_an_unfinished_identity_without_waiting() {
    let capture = LearnCapture::default();
    capture.arm();
    capture.offer(9, &[0x90, 61, 100]);
    capture.offer_with(1, &[0x90, 60, 100], &mut |state, previous, next| {
        assert_eq!(capture.captured(), None);
        capture.offer(2, &[0xb0, 7, 127]);
        state.compare_exchange(previous, next, Ordering::AcqRel, Ordering::Acquire)
    });
    assert_eq!(capture.captured(), Some((1, 0, Kind::Note, 60)));
}

#[test]
fn rearming_after_the_capture_cas_does_not_publish_an_old_identity() {
    let capture = LearnCapture::default();
    capture.arm();
    capture.offer_with(1, &[0x90, 60, 100], &mut |state, previous, next| {
        let result = state.compare_exchange(previous, next, Ordering::AcqRel, Ordering::Acquire);
        capture.cancel();
        capture.arm();
        capture.offer(2, &[0xb0, 7, 127]);
        result
    });
    assert_eq!(capture.captured(), None);
    capture.offer(2, &[0xb0, 7, 127]);
    assert_eq!(capture.captured(), Some((2, 0, Kind::Cc, 7)));
}

#[test]
fn an_offer_from_an_earlier_selection_cannot_capture_a_rearmed_target() {
    for cancel in [false, true] {
        let capture = LearnCapture::default();
        capture.arm();
        capture.offer_with(1, &[0x90, 60, 100], &mut |input, previous, encoded| {
            // Pause after reading target A's state, immediately before the capture CAS.
            if cancel {
                capture.cancel();
            }
            capture.arm();
            input.compare_exchange(previous, encoded, Ordering::AcqRel, Ordering::Acquire)
        });
        assert_eq!(
            capture.captured(),
            None,
            "old offer captured target B; cancel={cancel}"
        );
        assert!(capture.active());
        capture.offer(2, &[0xb0, 7, 127]);
        assert_eq!(capture.captured(), Some((2, 0, Kind::Cc, 7)));
    }
}

#[test]
fn map_mode_captures_future_controls_and_replaces_until_cancelled() {
    let capture = LearnCapture::default();
    assert!(!capture.active());
    capture.offer(1, &[0xb0, 7, 1]);
    assert_eq!(capture.captured(), None);
    capture.arm();
    assert!(capture.active());
    assert_eq!(capture.captured(), None);
    for bytes in [
        &[0xf8][..],
        &[0xb0, 128, 0],
        &[0xb0, 7, 128],
        &[0x80, 60, 100],
        &[0x90, 60, 0],
    ] {
        capture.offer(2, bytes);
    }
    assert_eq!(capture.captured(), None);
    capture.offer(42, &[0x93, 60, 1]);
    assert_eq!(capture.captured(), Some((42, 3, Kind::Note, 60)));
    capture.offer(99, &[0xb0, 7, 100]);
    assert_eq!(capture.captured(), Some((99, 0, Kind::Cc, 7)));
    capture.cancel();
    capture.offer(8, &[0xbf, 127, 0]);
    assert!(!capture.active());
    assert_eq!(capture.captured(), None);
    capture.arm();
    capture.offer(8, &[0xbf, 127, 0]);
    assert_eq!(capture.captured(), Some((8, 15, Kind::Cc, 127)));
}
