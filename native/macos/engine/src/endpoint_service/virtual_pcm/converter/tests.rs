mod alignment;
mod drift;
mod handoff;
use super::*;
use mixrelay_macos_transport::{
    application_clock::VERSION,
    client_pcm::{ClientPcmMixer, ClientPcmReader, ClientPcmSnapshot, ClientPcmWriter},
    endpoint_transport::EndpointTransportConfig,
    source_control::SourceRequest,
    source_registry::SourceRegistry,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
struct Clock {
    nanos: Arc<AtomicU64>,
}
impl mixrelay_time::MonotonicClock for Clock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(Duration::from_nanos(
            self.nanos.load(Ordering::Relaxed),
        ))
    }
}

/// A converter whose stream has begun on the source.
fn started(
    transport: &mut SourceTransport,
    clock: SourceClockSnapshot,
    native: (u32, usize),
) -> Converter {
    let mut converter = Converter::new(clock, native).unwrap();
    assert!(converter.restart(transport, clock));
    converter
}

fn source_token(id: &str) -> u64 {
    id.bytes().fold(14_695_981_039_346_656_037, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
    })
}

fn client_grants() -> (ClientPcmWriter, Vec<ClientPcmReader>) {
    client_grants_with_capacity(16_384)
}

fn client_grants_with_capacity(capacity: u32) -> (ClientPcmWriter, Vec<ClientPcmReader>) {
    let mut writer = ClientPcmWriter::new(
        EndpointTransportConfig {
            endpoint_id: [1; 16],
            capacity_frames: capacity,
            render_channels: 2,
            capture_channels: 2,
        },
        42,
    )
    .unwrap();
    assert!(writer.set_clock((2, 10_000), 48_000, true, true));
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    assert!(snapshot.push(&writer));
    // SAFETY: the snapshot retains producer-created grants throughout decoding.
    let queues = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    (writer, queues)
}

#[test]
fn shared_client_pcm_preserves_signal_and_silence_across_independent_reader_gaps() {
    let nanos = Arc::new(AtomicU64::new(0));
    let registry = crate::endpoint_service::virtual_pcm::test_storage::registry(Box::new(Clock {
        nanos: nanos.clone(),
    }));
    let mut request = SourceRequest {
        capacity_frames: 16_384,
        channels: 2,
        transport_version: VERSION,
        ..SourceRequest::default()
    };
    for (target, byte) in request.source_id.iter_mut().zip(b"virtual:fixture") {
        *target = std::ffi::c_char::from_ne_bytes([*byte]);
    }
    let token = source_token("virtual:fixture");
    let mut transport = SourceTransport::create(&registry, &request).unwrap();
    let mut output = [0.0; 256];
    registry.read_audio(token, &mut output, 128, 48_000);
    assert!(registry.set_clock_master(token, true));
    let clock = transport.clock().unwrap();
    let mut converter = started(&mut transport, clock, (48_000, 2));
    let (writer, queues) = client_grants();
    let mut mixer = ClientPcmMixer::default();
    let mut scratch = [0.0; 256];
    let mut input = [0.0; 256];
    let mut times = [0_u64; 128];
    let mut failures = 0;
    let measured = allocation_counter::measure(|| {
        // Each phase lasts beyond a ring wrap. A second consumer reads selected
        // partial windows; the source cursor must still return every frame.
        for block in 0..648_u32 {
            nanos.store(
                u64::from(block) * 128 * 1_000_000_000 / 48_000,
                Ordering::Relaxed,
            );
            let frame = 1_000_000 + u64::from(block) * 128;
            let audible = (168..328).contains(&block) || (488..648).contains(&block);
            for (index, pair) in input.chunks_exact_mut(2).enumerate() {
                let value = if audible {
                    f32::from(u16::try_from((frame + index as u64) % 30_000 + 1).unwrap())
                        / 32_768.0
                } else {
                    0.0
                };
                pair.copy_from_slice(&[value, if audible { -value } else { 0.0 }]);
            }
            assert!(writer.write(frame, &input, 128));
            // A second consumer's partial/skipped requests cannot alter this cursor.
            if block % 7 == 0 {
                let _ = writer.read(frame + 36, &mut scratch[..72], 36);
            }
            let window = mixer
                .window(&queues, |_| Some((frame + 128).div_ceil(48_000) * 48_000))
                .unwrap();
            assert_eq!(window.first, frame);
            let expected = input;
            ClientPcmMixer::mix(&queues, window, &mut input, &mut scratch).unwrap();
            assert_eq!(input.map(f32::to_bits), expected.map(f32::to_bits));
            for (index, time) in times.iter_mut().enumerate() {
                *time = 1_000_000_000 + (frame + index as u64) * 1_000_000_000 / 48_000;
            }
            let clock = transport.clock().unwrap();
            converter.observe(&transport, clock).unwrap();
            assert!(converter.flush(&mut transport).unwrap());
            converter.push(&mut transport, &mut input, &times).unwrap();
            assert!(converter.flush(&mut transport).unwrap());
            mixer.commit(window);
            transport.publish_measurements(128, 128, 0);
            let read = registry.read_at(token, &mut output, 48_000, times[0]);
            // Eight initial silent blocks establish the existing consumer latency
            // contract before testing every frame of both silence/signal orders.
            if block < 8 {
                assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 256]);
            } else if read != 128 || output.map(f32::to_bits) != expected.map(f32::to_bits) {
                failures += 1;
            }
        }
    });
    assert_eq!(
        failures, 0,
        "native source PCM must arrive once at its original acquisition coordinate"
    );
    assert_eq!(measured.count_total, 0);
}
