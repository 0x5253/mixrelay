use super::*;

#[test]
fn partial_bypass_reads_keep_the_original_frame_coordinates() {
    let mut config = config(1, 48_000, 48_000);
    config.clock_relation = ClockRelation::ProvenSameClock;
    config.priming_frames = 1;
    let mut bridge = AsyncClockBridge::new(config).unwrap();
    let pcm: [f32; 128] = std::array::from_fn(|i| i as f32);
    let times: [u64; 128] = std::array::from_fn(|i| 1_000_000_000 + i as u64 * 20_833);
    bridge.push_timed_interleaved(&pcm, &times).unwrap();
    let mut offset = 0;
    let mut output = [0.0; 17];
    let mut result = [0; 17];
    while offset < pcm.len() {
        let report = bridge
            .pull_timed_interleaved(&mut output, &mut result)
            .unwrap();
        let count = report.transferred_frames;
        assert_eq!(&output[..count], &pcm[offset..offset + count]);
        assert_eq!(&result[..count], &times[offset..offset + count]);
        offset += count;
    }
    bridge.reset(2, Discontinuity::StreamRestart);
    bridge.push_interleaved(&pcm).unwrap();
    bridge
        .pull_timed_interleaved(&mut output, &mut result)
        .unwrap();
    assert!(result.iter().all(|time| *time == 0));
}

#[test]
fn sinc_impulse_coordinates_survive_rate_conversion_and_arbitrary_pull_splits() {
    for (input_rate, output_rate) in [(48_000, 48_000), (44_100, 48_000), (48_000, 44_100)] {
        let mut config = config(1, input_rate, output_rate);
        config.priming_frames = 128;
        let mut bridge = AsyncClockBridge::new(config).unwrap();
        let mut input = [0.0; 128];
        let mut times = [0; 128];
        let mut output = [0.0; 37];
        let mut result = [0; 37];
        let mut peaks = [(0.0_f32, 0_u64); 10];
        for batch in 0..80 {
            for index in 0..128 {
                let position = batch * 128 + index;
                input[index] = if position % 1024 == 512 { 1.0 } else { 0.0 };
                times[index] =
                    1_000_000_000 + (position as f64 * 1e9 / f64::from(input_rate)).round() as u64;
            }
            bridge.push_timed_interleaved(&input, &times).unwrap();
            loop {
                let report = bridge
                    .pull_timed_interleaved(&mut output, &mut result)
                    .unwrap();
                for index in 0..report.transferred_frames {
                    if output[index] > 0.1 && result[index] >= 1_000_000_000 {
                        let position =
                            (result[index] - 1_000_000_000) as f64 * f64::from(input_rate) / 1e9;
                        let pulse = ((position - 512.0) / 1024.0).round().max(0.0) as usize;
                        if let Some(peak) = peaks.get_mut(pulse)
                            && output[index] > peak.0
                        {
                            *peak = (output[index], result[index]);
                        }
                    }
                }
                if report.transferred_frames == 0 {
                    break;
                }
            }
        }
        for (index, (level, observed)) in peaks.into_iter().enumerate() {
            assert!(
                level > 0.5,
                "{input_rate}->{output_rate}: missing impulse {index}"
            );
            let expected = 1_000_000_000
                + (((index * 1024 + 512) as f64 * 1e9) / f64::from(input_rate)).round() as u64;
            assert!(
                observed.abs_diff(expected) <= 1_000_000_000 / u64::from(output_rate),
                "{input_rate}->{output_rate}: {observed}/{expected}"
            );
        }
    }
}
