//! A native clock releases work once per period and retains PCM across a delayed wake.
use super::*;
use mixrelay_macos_transport::{
    client_pcm::{ClientPcmMixer, ClientPcmSnapshot, ClientPcmWriter},
    endpoint_transport::EndpointTransportConfig,
};

#[test]
fn waiting_follows_absolute_native_periods_without_accumulating_wake_lateness() {
    let mut elapsed = 0;
    let mut releases = 0;
    while elapsed < 42_666_667 && releases < 1_024 {
        let deadline = next(elapsed, 48_000).unwrap();
        let nanos = u64::try_from(deadline.as_nanos()).unwrap();
        assert!(nanos > elapsed);
        elapsed = nanos;
        releases += 1;
    }
    assert_eq!(releases, 16, "waiting must not poll between native periods");
    assert_eq!(elapsed, 42_666_667);
    assert_eq!(next(3_000_000, 48_000).unwrap().as_nanos(), 5_333_334);
    assert_eq!(next(8_000_000, 48_000).unwrap().as_nanos(), 10_666_667);
    assert!(next(1, 0).is_none());
}

#[test]
fn native_period_bounds_work_and_delayed_wakes_preserve_all_retained_pcm() {
    let mut writer = ClientPcmWriter::new(
        EndpointTransportConfig {
            endpoint_id: [2; 16],
            capacity_frames: 16_384,
            render_channels: 2,
            capture_channels: 2,
        },
        73,
    )
    .unwrap();
    assert!(writer.set_clock((3, 10_000), 48_000, true, true));
    let source: [f32; 4_096] = std::array::from_fn(|sample| {
        let value = f32::from(u16::try_from(sample / 2 + 1).unwrap()) / 4_096.0;
        if sample % 2 == 0 { value } else { -value }
    });
    assert!(writer.write(0, &source, 2_048));
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    assert!(snapshot.push(&writer));
    // SAFETY: the snapshot retains all producer-created grants throughout decoding.
    let queues = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    let mut mixer = ClientPcmMixer::default();
    let mut output = [0.0; 256];
    let mut scratch = [0.0; 256];
    let mut captured = Vec::with_capacity(source.len());
    let mut windows = 0;
    let measured = allocation_counter::measure(|| {
        // Frequent polls observe individual sample advances. A subsequent delayed
        // wake must drain the retained interval without skipping to the newest tick.
        for observed in (0..=1_024_u64).chain(std::iter::once(2_048)) {
            let elapsed = (observed * 1_000_000_000).div_ceil(48_000);
            while let Some(window) = mixer.window(&queues, |clock| frame(elapsed, clock.rate)) {
                let output = &mut output[..window.frames as usize * 2];
                ClientPcmMixer::mix(&queues, window, output, &mut scratch).unwrap();
                captured.extend_from_slice(output);
                mixer.commit(window);
                windows += 1;
            }
        }
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(captured.len(), source.len());
    for (actual, expected) in captured.iter().zip(source) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
    assert_eq!(
        windows, 16,
        "polling must not release one processing job per native sample"
    );
}
