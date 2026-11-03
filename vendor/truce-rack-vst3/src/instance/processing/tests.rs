use super::*;

#[test]
fn audio_blocks_must_fit_negotiated_geometry_and_sample_rate() {
    let mut output = EventList::new();
    let mut context = ProcessContext {
        sample_rate: 48000.0,
        max_block_size: 512,
        transport: None,
        output_events: &mut output,
    };
    let mut setup = ProcessSetup {
        processMode: 0,
        symbolicSampleSize: 0,
        maxSamplesPerBlock: 512,
        sampleRate: 48000.0,
    };
    for frames in [0, 1, 511, 512] {
        assert!(accepts_block(&setup, &context, frames));
    }
    for frames in [513, usize::MAX] {
        assert!(!accepts_block(&setup, &context, frames));
    }
    context.max_block_size = 128;
    assert!(!accepts_block(&setup, &context, 129));
    assert!(accepts_block(&setup, &context, 128));
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY, 44100.0] {
        context.sample_rate = rate;
        assert!(!accepts_block(&setup, &context, 128));
    }
    context.sample_rate = 48000.0;
    setup.maxSamplesPerBlock = -1;
    assert!(!accepts_block(&setup, &context, 0));
}
