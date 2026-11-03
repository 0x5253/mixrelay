use super::*;

fn geometry(render: u32, capture: u32) -> Geometry {
    Geometry {
        endpoint_id: [1; 16],
        render_channels: render,
        capture_channels: capture,
    }
}

fn add(mixer: &mut ClientMix, id: u32) -> bool {
    mixer
        .prepare(id)
        .is_some_and(|mut prepared| mixer.register(&mut prepared))
}

/// Detached queues, reserved like the native runtime's retired list.
fn retired() -> Vec<Queue> {
    Vec::with_capacity(RETIRED_CAPACITY)
}

fn mixer(render: u32, capture: u32) -> ClientMix {
    let mut mixer = ClientMix::new(geometry(render, capture)).unwrap();
    for id in [42, 43, 44] {
        assert!(add(&mut mixer, id));
    }
    mixer
}

#[test]
fn queue_admission_includes_frame_tags_and_rejects_invalid_geometry() {
    // Per-frame tags and headers count against the 1 GiB budget. Retention-sized
    // queues admitted only three 64-channel writers; playback-sized queues admit 62.
    // Each admitted queue is resident once prepared, so the bound is also the HAL's
    // worst-case client-PCM footprint per endpoint.
    assert_eq!(client_limit(MAX_PCM_BYTES, 64), 62);
    assert_eq!(client_limit(MAX_PCM_BYTES, 2), MAX_CLIENTS);
    assert_eq!(client_limit(MAX_PCM_BYTES, 0), MAX_CLIENTS);
    for channels in [65, u32::MAX] {
        assert_eq!(client_limit(MAX_PCM_BYTES, channels), 0);
    }
}

#[test]
fn registration_rejects_a_queue_prepared_for_another_endpoint_clock() {
    let mut first = mixer(2, 2);
    let other = mixer(2, 2);
    let foreign = other.prepare(99).unwrap();
    let revision = first.revision();
    let mut foreign = foreign;
    assert!(!first.register(&mut foreign));
    assert_eq!(first.revision(), revision);
    let mut capture_only = ClientMix::new(geometry(0, 2)).unwrap();
    assert!(!capture_only.register(&mut first.prepare(5).unwrap()));
    assert!(add(&mut capture_only, 5));
}

#[test]
fn rejected_registration_preserves_existing_pcm_and_membership() {
    let mut mixer = mixer(2, 2);
    assert!(mixer.render(42, 0, &[0.375; 16], 8).accepted);
    // Fill metadata slots without allocating another client PCM mapping.
    for id in 100..100 + u32::try_from(MAX_CLIENTS - mixer.clients.len()).unwrap() {
        mixer.clients.push(Client {
            id,
            rendered: AtomicBool::new(false),
            queue: None,
        });
    }
    let revision = mixer.revision();
    assert!(!add(&mut mixer, 999));
    assert_eq!(mixer.revision(), revision);
    let mut output = [9.0; 16];
    assert_eq!(mixer.capture(0, &mut output, 8).available, 8);
    assert_eq!(output.map(f32::to_bits), [0.375_f32.to_bits(); 16]);
}

#[test]
fn failed_native_start_cannot_publish_a_running_client_clock() {
    let mut mixer = mixer(2, 2);
    let mut clock = Clock::new(48_000, 1, 1).unwrap();
    assert!(mixer.publish_clock(&clock, true, &mut retired()));
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    assert!(mixer.append_snapshot(&mut snapshot));
    // SAFETY: the test retains the complete snapshot and every immutable grant header.
    let readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    assert!(
        readers
            .iter()
            .all(|reader| !reader.clock().unwrap().running)
    );
    assert!(clock.start(100));
    assert!(
        !mixer.publish_clock_after(&clock, true, &mut retired(), || {
            assert!(readers.iter().all(|reader| reader.clock().is_none()));
            false
        })
    );
    assert!(
        readers
            .iter()
            .all(|reader| reader.clock().is_none_or(|clock| !clock.running))
    );
    assert!(mixer.publish_clock(&clock, true, &mut retired()));
    assert!(readers.iter().all(|reader| reader.clock().unwrap().running));
}

#[test]
fn an_exhausted_queue_detaches_without_allocating_until_its_client_installs_a_new_one() {
    let mut mixer = mixer(2, 2);
    let mut clock = Clock::new(48_000, 1, 1).unwrap();
    let mut detached = retired();
    while !mixer.needs_queue(42) {
        clock.running = false;
        assert!(clock.start(1));
        assert!(mixer.publish_clock(&clock, true, &mut detached));
    }
    assert!(mixer.render(42, 0, &[0.375; 256], 128).accepted);
    let revision = mixer.revision();
    // The rate rearm cannot open another epoch: the queue leaves the published set and
    // waits in the reserved retired list, so the freeze neither allocates nor unmaps.
    let usage = allocation_counter::measure(|| {
        assert!(mixer.publish_clock(&clock, true, &mut detached));
    });
    assert_eq!(usage.count_total, 0);
    assert_eq!(detached.len(), 3);
    assert!(mixer.revision() > revision);
    let mut output = [9.0; 256];
    assert_eq!(mixer.capture(0, &mut output, 128).available, 0);
    assert!(!mixer.render(42, 0, &[0.25; 256], 128).accepted);
    // The client's next IO start maps a fresh queue on control and installs it.
    let mut prepared = mixer.prepare(42).unwrap();
    assert!(mixer.install(&mut prepared, &mut detached));
    assert!(mixer.publish_clock(&clock, false, &mut detached));
    assert!(mixer.render(42, 0, &[0.25; 256], 128).accepted);
    assert_eq!(mixer.capture(0, &mut output, 128).available, 128);
    assert_eq!(output.map(f32::to_bits), [0.25_f32.to_bits(); 256]);
    assert!(mixer.needs_queue(43));
}

#[test]
fn a_writer_in_flight_is_not_mistaken_for_an_exhausted_queue() {
    let mixer = mixer(2, 2);
    let queue = mixer.clients[0].queue.as_ref().unwrap();
    // IO-start preparation runs alongside IO, so a MixOutput callback may hold the claim.
    let claim = queue.hold_producer().unwrap();
    assert!(!queue.can_rearm());
    assert!(!mixer.needs_queue(42));
    drop(claim);
    assert!(queue.can_rearm());
}

#[test]
fn a_refused_endpoint_rearm_leaves_every_queue_and_the_clock_published() {
    let mut mixer = mixer(2, 2);
    let mut clock = Clock::new(48_000, 1, 1).unwrap();
    assert!(clock.start(1));
    assert!(mixer.publish_clock(&clock, true, &mut retired()));
    assert!(mixer.render(42, 0, &[0.375; 256], 128).accepted);
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    assert!(mixer.append_snapshot(&mut snapshot));
    // SAFETY: the test retains the complete snapshot and every immutable grant header.
    let readers = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
    let before = readers[0].clock().unwrap();
    let revision = mixer.revision();
    let mut faster = clock.clone();
    assert!(faster.set_rate(96_000, 2));
    assert!(!mixer.publish_clock_after(&faster, true, &mut retired(), || false));
    assert_eq!(mixer.revision(), revision);
    assert_eq!(readers[0].clock(), Some(before));
    let mut output = [9.0; 256];
    assert_eq!(mixer.capture(0, &mut output, 128).available, 128);
}

#[test]
fn a_new_geometry_detaches_queues_of_the_old_shape_without_mapping() {
    let mut mixer = mixer(2, 2);
    let mut detached = retired();
    let revision = mixer.revision();
    let usage = allocation_counter::measure(|| mixer.reconfigure(geometry(4, 2), &mut detached));
    assert_eq!(usage.count_total, 0);
    assert_eq!(detached.len(), 3);
    assert!(mixer.revision() > revision);
    assert!([42, 43, 44].iter().all(|id| mixer.needs_queue(*id)));
    let mut snapshot = ClientPcmSnapshot::new().unwrap();
    assert!(mixer.append_snapshot(&mut snapshot));
    // SAFETY: the snapshot retains every grant it holds, here none.
    assert!(
        !matches!(unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }, Ok(readers) if !readers.is_empty())
    );
}

#[test]
fn a_prepared_client_output_survives_missing_postmix_cycles_and_different_capture_widths() {
    let mixer = mixer(2, 2);
    let source: Vec<_> = (0..1024_u16)
        .map(|value| f32::from(value + 1) / 2048.0)
        .collect();
    // The observed failed host sequence omitted postmix [167020,167396).
    // This client supplied all of [166884,167396) before any input requested it.
    assert!(mixer.render(42, 166_884, &source, 512).accepted);
    let mut short = [0.0; 256];
    assert_eq!(mixer.capture(166_884, &mut short, 128).available, 128);
    assert_eq!(
        short.map(f32::to_bits),
        source[..256]
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
            .as_slice()
    );
    let mut full = [0.0; 1024];
    assert_eq!(mixer.capture(166_884, &mut full, 512).available, 512);
    assert_eq!(
        full.map(f32::to_bits),
        source
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
            .as_slice()
    );
}

#[test]
fn overlapping_clients_mix_once_and_registration_reuse_cannot_replay_or_reset_its_peer() {
    let mut mixer = mixer(2, 4);
    assert!(mixer.render(42, 100, &[0.125; 16], 8).accepted);
    assert!(mixer.render(43, 100, &[0.25; 16], 8).accepted);
    let mut output = [0.0; 32];
    assert_eq!(mixer.capture(100, &mut output, 8).available, 8);
    assert!(
        output
            .chunks_exact(4)
            .all(|frame| frame == [0.375, 0.375, 0.0, 0.0])
    );
    mixer.remove(43);
    assert!(add(&mut mixer, 43));
    assert_eq!(mixer.capture(100, &mut output, 8).available, 8);
    assert!(
        output
            .chunks_exact(4)
            .all(|frame| frame == [0.125, 0.125, 0.0, 0.0])
    );
    assert!(mixer.render(43, 100, &[0.5; 16], 8).accepted);
    assert_eq!(mixer.capture(100, &mut output, 8).available, 8);
    assert!(
        output
            .chunks_exact(4)
            .all(|frame| frame == [0.625, 0.625, 0.0, 0.0])
    );
    mixer.remove(42);
    assert_eq!(mixer.capture(100, &mut output, 8).available, 8);
    assert!(
        output
            .chunks_exact(4)
            .all(|frame| frame == [0.5, 0.5, 0.0, 0.0])
    );
}

#[test]
fn a_finished_client_supplies_zero_beyond_its_window_while_its_peer_continues() {
    let mixer = mixer(2, 2);
    assert!(mixer.render(42, 0, &[0.125; 16], 8).accepted);
    assert!(mixer.render(43, 0, &[0.25; 8], 4).accepted);
    let mut output = [9.0; 16];
    let outcome = mixer.capture(0, &mut output, 8);
    assert!(outcome.accepted);
    assert_eq!(outcome.available, 8);
    assert_eq!(
        output.map(f32::to_bits),
        [
            0.375, 0.375, 0.375, 0.375, 0.375, 0.375, 0.375, 0.375, 0.125, 0.125, 0.125, 0.125,
            0.125, 0.125, 0.125, 0.125
        ]
        .map(f32::to_bits)
    );
    assert!(!mixer.render(999, 0, &[0.25; 16], 8).accepted);
    assert!(!mixer.render(42, 4, &[0.25; 16], 8).accepted);
    assert_eq!(mixer.capture(u64::MAX, &mut output, 8).available, 0);
    assert_eq!(output.map(f32::to_bits), [0; 16]);
}

#[test]
fn tiled_mix_preserves_sparse_frame_coverage_across_wrap_and_channel_remapping() {
    for (render, capture) in [(2, 2), (2, 7), (7, 2), (64, 64)] {
        let mixer = mixer(render, capture);
        // Span a ring wrap and both sides of successive copy-tile boundaries.
        let start = 16_368;
        let first = vec![0.125; 31 * render as usize];
        let second = vec![0.25; 33 * render as usize];
        assert!(mixer.render(42, start, &first, 31).accepted);
        assert!(mixer.render(43, start + 30, &second, 33).accepted);
        // Digital zero contributes valid coverage after a one-frame hole at 63.
        assert!(
            mixer
                .render(44, start + 64, &vec![0.0; render as usize], 1)
                .accepted
        );
        let mut output = vec![9.0; 67 * capture as usize];
        assert_eq!(mixer.capture(start, &mut output, 67).available, 64);
        for (frame, samples) in output.chunks_exact(capture as usize).enumerate() {
            let expected: f32 = match frame {
                0..30 => 0.125,
                30 => 0.375,
                31..63 => 0.25,
                _ => 0.0,
            };
            for (channel, sample) in samples.iter().enumerate() {
                assert_eq!(
                    sample.to_bits(),
                    if channel < render as usize {
                        expected.to_bits()
                    } else {
                        0
                    }
                );
            }
        }
    }
}

#[test]
fn single_and_multiple_client_callback_transfers_never_allocate_or_free() {
    for (capture, contributors) in [(2, 1), (2, 2), (4, 1), (4, 2)] {
        let mixer = mixer(2, capture);
        let mut captured = [0.0; 512];
        let usage = allocation_counter::measure(|| {
            for cycle in 0..100_u64 {
                assert!(mixer.render(42, cycle * 128, &[0.125; 256], 128).accepted);
                if contributors == 2 {
                    assert!(mixer.render(43, cycle * 128, &[0.25; 256], 128).accepted);
                }
                assert_eq!(
                    mixer
                        .capture(cycle * 128, &mut captured[..capture as usize * 128], 128)
                        .available,
                    128
                );
            }
        });
        assert_eq!(usage.count_total, 0);
        assert_eq!(usage.bytes_total, 0);
        assert_eq!(usage.count_current, 0);
    }
}

#[test]
#[ignore = "offline client mixing throughput measurement; run explicitly with --release --ignored --nocapture"]
fn client_mixing_throughput() {
    const CYCLES: u64 = 2000;
    const FRAMES: u32 = 512;
    for (render, capture, contributors) in [(2, 2, 1), (2, 2, 2), (2, 2, 8), (2, 7, 2), (64, 64, 2)]
    {
        let mut mixer = ClientMix::new(geometry(render, capture)).unwrap();
        for client in 1..=contributors {
            assert!(add(&mut mixer, client));
        }
        let input = vec![0.125; (FRAMES * render) as usize];
        let mut output = vec![0.0; (FRAMES * capture) as usize];
        let start = std::time::Instant::now();
        for cycle in 0..CYCLES {
            let frame = cycle * u64::from(FRAMES);
            for client in 1..=contributors {
                assert!(mixer.render(client, frame, &input, FRAMES).accepted);
            }
            assert_eq!(mixer.capture(frame, &mut output, FRAMES).available, FRAMES);
            std::hint::black_box(&output);
        }
        let elapsed = start.elapsed();
        assert!(
            output
                .chunks_exact(capture as usize)
                .all(|frame| frame
                    .iter()
                    .enumerate()
                    .all(|(channel, sample)| sample.to_bits()
                        == if channel < render as usize {
                            0.125 * f32::from(u16::try_from(contributors).unwrap())
                        } else {
                            0.0
                        }
                        .to_bits()))
        );
        println!(
            "client_mix arch={} render={render} capture={capture} contributors={contributors} cycles={CYCLES} frames={FRAMES} elapsed_us={}",
            std::env::consts::ARCH,
            elapsed.as_micros()
        );
    }
}

#[test]
fn a_widened_geometry_maps_and_installs_queues_only_within_the_pcm_budget() {
    let narrow = buffer::mr_shared_audio_buffer_required_bytes(QUEUE_FRAMES, 2);
    let wide = buffer::mr_shared_audio_buffer_required_bytes(QUEUE_FRAMES, 16);
    // Three 2-channel queues fit, but only one 16-channel queue.
    let budget = (3 * narrow).max(wide);
    assert_eq!(budget / wide, 1);
    let mut mixer = ClientMix::new(geometry(2, 2)).unwrap().with_budget(budget);
    for id in [42, 43, 44] {
        assert!(add(&mut mixer, id));
    }
    let mut detached = retired();
    mixer.reconfigure(geometry(16, 2), &mut detached);
    // Registration-time admission no longer bounds what the wider shape maps.
    let mut first = mixer.prepare(42).unwrap();
    let mut second = mixer.prepare(43).unwrap();
    assert!(mixer.install(&mut first, &mut detached));
    assert!(mixer.prepare(44).is_none());
    assert!(!mixer.install(&mut second, &mut detached));
    assert!(second.into_queue().is_some());
    // The installed client may still replace its own queue.
    let mut replacement = mixer.prepare(42).unwrap();
    assert!(mixer.install(&mut replacement, &mut detached));
}
