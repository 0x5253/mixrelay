use super::*;

#[test]
fn all_channel_widths_preserve_three_owner_queues_and_recovery_epochs() {
    for channels in 1..=64 {
        let input = owner(true, channels);
        let output = owner(false, channels);
        let mut source = samples(channels, 64);
        let mut destination = vec![0.0; source.len()];
        let mut scratch = destination.clone();
        let mut list = buffers(&mut source, channels, 64, false);
        process(&input, &mut list);
        // SAFETY: every test role runs serially with complete disjoint PCM spans.
        unsafe {
            assert_eq!(input.progress.native_frames.load(Acquire), 64);
            assert_eq!(input.graph_ring.stats().queued, 0);
            assert_eq!(queues::bridge_read(&input, scratch.as_mut_ptr(), 64), 64);
            assert_pcm(&scratch, &source);
            assert_eq!(queues::bridge_read(&input, scratch.as_mut_ptr(), 64), 0);
            assert_eq!(
                queues::graph_read(&input, destination.as_mut_ptr(), 64, 1, (0, None)).frames,
                0
            );
            for _ in 0..3 {
                assert_eq!(queues::bridge_write(&input, source.as_ptr(), 64), 64);
            }
            assert_eq!(
                queues::graph_read(&input, destination.as_mut_ptr(), 64, 1, (0, None)).frames,
                64
            );
            assert_pcm(&destination, &source);
            assert_eq!(
                queues::graph_read(&input, destination.as_mut_ptr(), 64, 2, (0, None)).frames,
                0
            );
            assert_eq!(input.graph_ring.stats().queued, 0);
            assert_eq!(input.progress.graph_frames.load(Acquire), 192);
            assert_eq!(queues::graph_write(&output, source.as_ptr(), 64, 0), 64);
            assert_eq!(output.native_ring.stats().queued, 0);
            assert_eq!(queues::bridge_read(&output, scratch.as_mut_ptr(), 64), 64);
            assert_pcm(&scratch, &source);
            assert_eq!(queues::bridge_write(&output, scratch.as_ptr(), 64), 64);
        }
        let mut list = buffers(&mut destination, channels, 64, false);
        process(&output, &mut list);
        assert_pcm(&destination, &source);
        output
            .progress
            .output_epoch
            .store(1, std::sync::atomic::Ordering::Release);
        process(&output, &mut list);
        assert!(destination.iter().all(|sample| sample.to_bits() == 0));
        assert_eq!(output.native_ring.stats().queued, 0);
        assert_eq!(output.progress.native_quantum.load(Acquire), 64);
        assert_eq!(output.progress.graph_quantum.load(Acquire), 64);
        assert_eq!(output.progress.native_frames.load(Acquire), 128);
        // SAFETY: serialized worker owns the graph output consumer cursor.
        unsafe {
            queues::graph_write(&output, source.as_ptr(), 64, 0);
            queues::bridge_discard(&output);
        }
        assert_eq!(output.graph_ring.stats().queued, 0);
    }
}

fn pcm_case(channels: u32, planar: bool) {
    let input = owner(true, channels);
    let output = owner(false, channels);
    let source = samples(channels, 64);
    let mut hardware = vec![0.0; source.len()];
    for frame in 0..64 {
        for channel in 0..channels as usize {
            hardware[if planar {
                channel * 64 + frame
            } else {
                frame * channels as usize + channel
            }] = source[frame * channels as usize + channel];
        }
    }
    let mut list = buffers(&mut hardware, channels, 64, planar);
    process(&input, &mut list);
    let mut copy = vec![0.0; source.len()];
    // SAFETY: sole worker consumer/producer; full disjoint test PCM allocations.
    unsafe {
        assert_eq!(queues::bridge_read(&input, copy.as_mut_ptr(), 64), 64);
        assert_pcm(&copy, &source);
        assert_eq!(queues::bridge_write(&output, copy.as_ptr(), 64), 64);
    }
    process(&output, &mut list);
    for frame in 0..64 {
        for channel in 0..channels as usize {
            assert_eq!(
                hardware[if planar {
                    channel * 64 + frame
                } else {
                    frame * channels as usize + channel
                }]
                .to_bits(),
                source[frame * channels as usize + channel].to_bits()
            );
        }
    }
    // SAFETY: no registered callback; fixture runs synchronously.
    unsafe {
        (*output.callback.get()).prime = true;
    }
    process(&output, &mut list);
    assert!(hardware.iter().all(|value| value.to_bits() == 0));
    list.buffers[0].channels = 0;
    process(&output, &mut list);
    assert!(output.progress.invalid.load(Acquire));
    let ticks = output.progress.ticks.load(Acquire);
    list.buffers[0].channels = if planar { 1 } else { channels };
    hardware.fill(1.0);
    process(&output, &mut list);
    assert_eq!(output.progress.ticks.load(Acquire), ticks);
    assert!(hardware.iter().all(|value| value.to_bits() == 0));
}

#[test]
fn callback_layouts_priming_and_terminal_invalid_latch() {
    for channels in [1, 2, 8, 16, 32, 64] {
        pcm_case(channels, false);
        pcm_case(channels, true);
    }
}

#[test]
fn mixed_stream_subset_preserves_selected_channels_and_silences_neighbors() {
    let input = owner(true, 4);
    let output = owner(false, 4);
    let mut hardware = [0.0; 64 * 6];
    let mut list = buffers(&mut hardware, 6, 64, false);
    list.count = 3;
    let mut offset = 0;
    for (index, width) in [1, 2, 3].into_iter().enumerate() {
        for frame in 0..64 {
            for channel in 0..width {
                hardware[offset * 64 + frame * width + channel] =
                    sample(offset + channel + 1) / 128.0;
            }
        }
        // SAFETY: each stream owns its disjoint 64*width subspan.
        list.buffers[index] = AudioBuffer {
            channels: u32::try_from(width).unwrap(),
            byte_size: u32::try_from(64 * width * 4).unwrap(),
            data: unsafe { hardware.as_mut_ptr().add(offset * 64) },
        };
        offset += width;
    }
    process(&input, &mut list);
    let mut graph = [0.0; 64 * 4];
    // SAFETY: worker role exclusively consumes/produces complete disjoint spans.
    unsafe {
        assert_eq!(queues::bridge_read(&input, graph.as_mut_ptr(), 64), 64);
        assert_eq!(queues::bridge_write(&output, graph.as_ptr(), 64), 64);
    }
    assert_pcm(&graph, &samples(4, 64));
    process(&output, &mut list);
    offset = 0;
    for width in [1, 2, 3] {
        for frame in 0..64 {
            for channel in 0..width {
                let expected = if offset + channel < 4 {
                    sample(offset + channel + 1) / 128.0
                } else {
                    0.0
                };
                assert_eq!(
                    hardware[offset * 64 + frame * width + channel].to_bits(),
                    expected.to_bits()
                );
            }
        }
        offset += width;
    }
}

fn aggregate_slice(offset: u32, channels: u32, input: bool) {
    let io = owner(input, channels);
    // SAFETY: fixture has no native registration; direct callback selection is the
    // resolved aggregate projection, without inventing a dangling aggregate owner.
    unsafe {
        (*io.callback.get()).selection = BufferSelection {
            channels,
            offset,
            total: 128,
            limit: 2048,
        };
    }
    let mut hardware: Vec<f32> = (0..512).map(sample).collect();
    let mut selected = Vec::new();
    for frame in 0..4 {
        for channel in 0..channels {
            selected.push(sample(frame * 128 + offset + channel));
        }
    }
    let mut list = buffers(&mut hardware, 64, 4, false);
    list.count = 2;
    // Pack two streams with the same global frame/channel values.
    for frame in 0..4 {
        for channel in 0..64 {
            hardware[frame * 64 + channel] = sample(frame * 128 + channel);
            hardware[256 + frame * 64 + channel] = sample(frame * 128 + channel + 64);
        }
    }
    // SAFETY: second stream is the disjoint second half of 512 initialized samples.
    list.buffers[1] = AudioBuffer {
        channels: 64,
        byte_size: 1024,
        data: unsafe { hardware.as_mut_ptr().add(256) },
    };
    // SAFETY: serialized output worker owns this ring's producer cursor.
    if !input {
        unsafe {
            assert_eq!(queues::bridge_write(&io, selected.as_ptr(), 4), 4);
        }
    }
    process(&io, &mut list);
    assert!(!io.progress.invalid.load(Acquire));
    if input {
        let mut read = vec![0.0; selected.len()];
        // SAFETY: sole input worker consumer and exact-sized writable output.
        unsafe {
            assert_eq!(queues::bridge_read(&io, read.as_mut_ptr(), 4), 4);
        }
        assert_pcm(&read, &selected);
    } else {
        for frame in 0..4 {
            for channel in 0..128 {
                let position = if channel < 64 {
                    frame * 64 + channel
                } else {
                    256 + frame * 64 + channel - 64
                };
                let expected = if (offset as usize..(offset + channels) as usize).contains(&channel)
                {
                    sample(frame * 128 + channel)
                } else {
                    0.0
                };
                assert_eq!(hardware[position].to_bits(), expected.to_bits());
            }
        }
    }
    list.count = 1;
    process(&io, &mut list);
    assert!(io.progress.invalid.load(Acquire));
}

#[test]
fn aggregate_slices_cover_offsets_and_member_disappearance() {
    for offset in [1, 13, 33, 64] {
        for channels in [1, 2, 7, 32, 64] {
            aggregate_slice(offset, channels, true);
            aggregate_slice(offset, channels, false);
        }
    }
}
