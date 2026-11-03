//! Capture coordinates and PCM enter each acknowledged producer epoch together.
use super::*;

#[test]
fn capture_frontier_and_reads_exclude_preparation_and_preserve_fresh_recovery_pcm() {
    let io = owner(true, 1);
    let pointer = ptr::from_ref(&io).cast_mut();
    let mut output = [9.0; 32];
    let times = |first| std::array::from_fn::<_, 32, _>(|i| first + i as u64 * 20_833);
    let measured = allocation_counter::measure(|| {
        // SAFETY: unpublished owner, serialized graph/bridge roles, complete disjoint spans.
        unsafe {
            assert_eq!(
                bridge_write_timed(pointer, &[0.25; 32], &times(1_000_000_000)),
                32
            );
            assert_eq!(input_time(pointer, 32), Some(1_000_000_000));
            queues::bridge_discard(&io);
            assert_eq!(input_time(pointer, 32), None);
            assert_eq!(
                bridge_write_timed(pointer, &[0.5; 32], &times(2_000_000_000)),
                32
            );
            assert_eq!(input_time(pointer, 32), Some(2_000_000_000));
            let read = graph_read_at(pointer, &mut output, 1, (0, 2_000_000_000, 48_000));
            assert_eq!(read.frames, 32);
            assert!(read.initial_epoch && !read.discontinuity);
            assert_eq!(output.map(f32::to_bits), [0.5_f32.to_bits(); 32]);
            assert_eq!(
                bridge_write_timed(pointer, &[0.75; 32], &times(3_000_000_000)),
                32
            );
            queues::bridge_discard(&io);
            assert_eq!(input_time(pointer, 32), None);
            assert_eq!(
                bridge_write_timed(pointer, &[0.125; 32], &times(4_000_000_000)),
                32
            );
            assert_eq!(input_time(pointer, 32), Some(4_000_000_000));
            let read = graph_read_at(pointer, &mut output, 2, (0, 4_000_000_000, 48_000));
            assert_eq!(read.frames, 32);
            assert!(!read.initial_epoch && read.discontinuity);
            assert_eq!(output.map(f32::to_bits), [0.125_f32.to_bits(); 32]);
        }
    });
    assert_eq!(measured.count_total, 0);
}
