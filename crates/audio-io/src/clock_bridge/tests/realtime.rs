use super::*;

#[test]
fn prepared_audio_paths_including_feedback_and_reset_do_not_allocate_or_free() {
    for channels in [1, 2, 8, 16, 32, 64] {
        let mut bridge = AsyncClockBridge::new(config(channels, 44_100, 48_000)).unwrap();
        let input = vec![0.25; 128 * channels];
        let mut output = vec![0.0; 256 * channels];
        let feedback = ClockFeedback {
            queue: FeedbackQueue::Output,
            epoch: 0,
            queued_frames: 1200,
            target_frames: 1024,
            elapsed_frames: 128,
            clock_running: true,
        };
        let measured = allocation_counter::measure(|| {
            for iteration in 0..100 {
                bridge.push_interleaved(&input).unwrap();
                bridge.pull_interleaved(&mut output).unwrap();
                bridge.observe_clock(feedback).unwrap();
                if iteration % 20 == 0 {
                    bridge.reset(0, Discontinuity::TransportLoss);
                }
            }
        });
        assert_eq!(
            measured.count_total, 0,
            "{channels} channels RT allocations"
        );
        // With zero allocations, a negative net count detects freeing storage
        // prepared before measurement; checking only count_total would miss it.
        assert_eq!(measured.count_current, 0, "{channels} channels RT frees");
        eprintln!(
            "RT {channels}ch 44.1->48k: 100 push/pull/feedback cycles +5 resets = 0 heap operations"
        );
    }
}

#[test]
fn extreme_supported_ratios_and_channel_counts_preserve_bounded_shapes() {
    for channels in [1, 64] {
        for (from, to) in [(8_000, 384_000), (384_000, 8_000)] {
            let mut bridge = AsyncClockBridge::new(config(channels, from, to)).unwrap();
            let input = vec![0.0; 128 * channels];
            let mut output = vec![1.0; 4096 * channels];
            for _ in 0..8 {
                bridge.push_interleaved(&input).unwrap();
                for _ in 0..3 {
                    let report = bridge.pull_interleaved(&mut output).unwrap();
                    assert!(report.transferred_frames <= 4096);
                    assert!(output.iter().all(|sample| *sample == 0.0));
                    assert!(report.queued_input_frames <= 8192);
                }
            }
        }
    }
}
