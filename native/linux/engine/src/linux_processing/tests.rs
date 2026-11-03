//! Native output completion across graph slices.
use super::*;

fn blocks() -> CycleBlocks {
    CycleBlocks::new(
        ProcessCycle {
            clock: mixrelay_audio_io::process::ClockDomain {
                authority: ClockAuthority::AudioServer,
                id: 26,
                epoch: 1,
            },
            sample_rate: 48_000,
            frames: 512,
            first_frame: 0,
            monotonic_ns: Some(0),
            discontinuity: false,
        },
        mixrelay_audio_io::session::SessionFormat {
            sample_rate: 48_000,
            block_frames: 128,
        },
    )
    .unwrap()
}

#[test]
fn output_release_follows_all_four_slices_once() {
    let mut counts = (0, 0);
    process_blocks_and_release(
        &mut counts,
        Some(blocks()),
        |counts, block| {
            assert_eq!(block.frames, 128);
            counts.0 += 1;
            assert_eq!(counts.1, 0);
            Ok(())
        },
        |counts| {
            assert_eq!(counts.0, 4);
            counts.1 += 1;
        },
    )
    .unwrap();
    assert_eq!(counts, (4, 1));
    counts = (0, 0);
    process_blocks_and_release(
        &mut counts,
        None,
        |_, _| panic!("zero render budget has no DSP slice"),
        |counts| counts.1 += 1,
    )
    .unwrap();
    assert_eq!(counts, (0, 1));
}

#[test]
fn failed_slice_does_not_release_partial_cycle() {
    let mut counts = (0, 0);
    let result = process_blocks_and_release(
        &mut counts,
        Some(blocks()),
        |counts, _| {
            counts.0 += 1;
            if counts.0 == 2 {
                Err(Failure::InvalidTransfer)
            } else {
                Ok(())
            }
        },
        |counts| counts.1 += 1,
    );
    assert_eq!(result, Err(Failure::InvalidTransfer));
    assert_eq!(counts, (2, 0));
}
