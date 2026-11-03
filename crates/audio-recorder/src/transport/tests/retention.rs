//! Delayed writer behavior at product rates, without a wall-clock sleep.
use super::*;

#[test]
fn four_seconds_of_quiet_and_signal_survive_a_suspended_writer_at_every_rate() {
    for &rate in mixrelay_audio_io::formats::SAMPLE_RATES {
        let shared = Transport::new(2, std::thread::yield_now).unwrap();
        let tap = RecorderTap::new(Arc::clone(&shared));
        let mut packet = Packet::new(2).unwrap();
        let frames = rate as usize * mixrelay_audio_io::retention::SECONDS as usize;
        let mut input = [0.0; 512];
        shared.gate.store(1, Ordering::SeqCst);
        let measured = allocation_counter::measure(|| {
            for first in (0..frames).step_by(256) {
                let count = (frames - first).min(256);
                for (offset, frame) in input[..count * 2].chunks_exact_mut(2).enumerate() {
                    let value = sample(first + offset, rate);
                    frame.copy_from_slice(&[value, -value]);
                }
                tap.capture(&input[..count * 2], rate);
            }
            assert_eq!(shared.cutoff(), None);
            let mut read = 0;
            for _ in 0..PACKETS {
                if !shared.read_packet(&mut packet) {
                    break;
                }
                assert_eq!(packet.rate, rate);
                for frame in packet.samples[..packet.frames * 2].chunks_exact(2) {
                    let expected = sample(read, rate);
                    assert_eq!(frame[0].to_bits(), expected.to_bits());
                    assert_eq!(frame[1].to_bits(), (-expected).to_bits());
                    read += 1;
                }
            }
            assert!(!shared.read_packet(&mut packet));
            assert_eq!(read, frames);
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
    }
}

fn sample(frame: usize, rate: u32) -> f32 {
    if (frame / (rate as usize / 4)).is_multiple_of(2) {
        0.0
    } else {
        f32::from_bits(0x3f00_0000 + u32::try_from(frame).unwrap())
    }
}
