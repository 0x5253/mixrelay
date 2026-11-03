use super::Take;
use crate::recording_files::memory::MemoryFiles;
use crate::{Encoding, RecorderAction, RecorderRequest};
use std::path::Path;

fn take(
    files: &mut MemoryFiles,
    path: &Path,
    encoding: Encoding,
    channels: u16,
) -> Take<MemoryFiles> {
    Take::new(
        &RecorderRequest {
            action: RecorderAction::Record,
            path: Some(path.to_str().unwrap().into()),
            sample_rate: Some(48_000),
            encoding: Some(encoding),
        },
        channels,
        48_000,
        files,
    )
    .unwrap()
}

fn pcm_words(files: &MemoryFiles, path: &Path) -> Vec<u32> {
    let mut reader = files.reader(path);
    if reader.spec().sample_format == hound::SampleFormat::Float {
        reader
            .samples::<f32>()
            .map(|sample| sample.unwrap().to_bits())
            .collect()
    } else {
        reader
            .samples::<i32>()
            .map(|sample| u32::from_ne_bytes(sample.unwrap().to_ne_bytes()))
            .collect()
    }
}

#[test]
fn single_rf64_file_preserves_exact_pcm_across_flush_and_resume() {
    for channels in [1, 2, 8] {
        let directory = Path::new("/recorder-exact-rf64");
        let mut files = MemoryFiles::default();
        let path = directory.join("take.wav");
        let mut take = take(&mut files, &path, Encoding::Float32, channels);
        let input: Vec<f32> = (0..channels * 6)
            .map(|sample| f32::from(sample) / 64.0)
            .collect();
        let boundary = usize::from(channels) * 3;
        take.write(&input[..boundary], 48_000).unwrap();
        take.flush_segment().unwrap();
        take.prepare_rate(48_000).unwrap();
        take.write(&input[boundary..], 48_000).unwrap();
        assert_eq!(take.finish().unwrap(), 6);
        assert_eq!(files.len(), 1);
        let reader = files.reader(&path);
        assert_eq!((reader.spec().channels, reader.duration()), (channels, 6));
        assert_eq!(
            pcm_words(&files, &path),
            input
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn packet_chunking_preserves_src_fractional_tail_and_tpdf_state() {
    let directory = Path::new("/recorder-converted-rf64");
    let mut files = MemoryFiles::default();
    for encoding in [Encoding::Pcm16, Encoding::Pcm24, Encoding::Float32] {
        let single_path = directory.join(format!("single-{encoding:?}.wav"));
        let chunked_path = directory.join(format!("chunked-{encoding:?}.wav"));
        let mut single = take(&mut files, &single_path, encoding, 2);
        let mut chunked = take(&mut files, &chunked_path, encoding, 2);
        for (rate, frames) in [(44_100, 37_i16), (96_000, 19), (48_000, 3)] {
            let samples: Vec<f32> = (0..frames)
                .flat_map(|index| {
                    let value = f32::from(index + 1) / 128.0;
                    [value, -value]
                })
                .collect();
            single.write(&samples, rate).unwrap();
            for frame in samples.chunks_exact(2) {
                chunked.write(frame, rate).unwrap();
            }
        }
        assert_eq!(chunked.finish().unwrap(), single.finish().unwrap());
        assert_eq!(
            pcm_words(&files, &chunked_path),
            pcm_words(&files, &single_path)
        );
    }
}

#[test]
fn record_and_resume_prepare_the_observed_converter_before_pcm_admission() {
    let directory = Path::new("/recorder-converter-preparation");
    let mut files = MemoryFiles::default();
    let mut take = Take::new(
        &RecorderRequest {
            action: RecorderAction::Record,
            path: Some(directory.join("prepared.wav").to_str().unwrap().into()),
            sample_rate: Some(96_000),
            encoding: Some(Encoding::Float32),
        },
        1,
        48_000,
        &mut files,
    )
    .unwrap();
    assert_eq!(take.segment.as_ref().unwrap().rate, 48_000);
    assert!(take.segment.as_ref().unwrap().resampler.is_some());
    // A clock changed after preparation: the packet's own rate remains the
    // authority, rather than retagging it with the observed Record-time rate.
    take.write(&[0.5], 44_100).unwrap();
    assert_eq!(take.segment.as_ref().unwrap().rate, 44_100);
    take.flush_segment().unwrap();
    assert!(take.segment.is_none());
    take.prepare_rate(48_000).unwrap();
    assert_eq!(take.segment.as_ref().unwrap().rate, 48_000);
    take.write(&[0.25], 48_000).unwrap();
    assert_eq!(take.finish().unwrap(), 4);
}

#[test]
fn one_frame_conversion_tails_have_exact_duration_for_all_supported_rate_pairs() {
    let directory = Path::new("/recorder-rate-pairs");
    let mut files = MemoryFiles::default();
    for &from in mixrelay_audio_io::formats::SAMPLE_RATES {
        for &to in mixrelay_audio_io::formats::SAMPLE_RATES {
            let path = directory.join(format!("{from}-{to}.wav"));
            let request = RecorderRequest {
                action: RecorderAction::Record,
                path: Some(path.to_str().unwrap().to_owned()),
                sample_rate: Some(to),
                encoding: Some(Encoding::Float32),
            };
            let mut take = Take::new(&request, 1, from, &mut files).unwrap();
            take.write(&[0.5], from).unwrap();
            let expected = (u64::from(to) + u64::from(from / 2)) / u64::from(from);
            assert_eq!(take.finish().unwrap(), expected);
            assert_eq!(u64::from(files.reader(path).duration()), expected);
        }
    }
}

#[test]
fn sample_rate_changes_flush_short_segments_and_keep_one_fixed_wav_rate() {
    let mut files = MemoryFiles::default();
    let path = Path::new("/recorder-rate/take.wav");
    let mut take = take(&mut files, path, Encoding::Float32, 1);
    take.write(&[0.25; 441], 44_100).unwrap();
    take.write(&[0.25; 960], 96_000).unwrap();
    take.write(&[0.25; 17], 48_000).unwrap();
    let frames = take.finish().unwrap();
    let mut reader = files.reader(path);
    let spec = reader.spec();
    let samples: Vec<f32> = reader.samples().map(Result::unwrap).collect();
    assert_eq!(
        (spec.sample_rate, frames, samples.len()),
        (48_000, 977, 977)
    );
    assert!(samples.iter().all(|sample| sample.is_finite()));
    assert!(
        samples[100..400]
            .iter()
            .all(|sample| (*sample - 0.25).abs() < 0.002)
    );
    assert_eq!(&samples[960..], &[0.25; 17]);
}

#[test]
fn integer_encodings_clip_and_dither_at_the_final_quantizer() {
    let mut files = MemoryFiles::default();
    for (encoding, bits) in [(Encoding::Pcm16, 16), (Encoding::Pcm24, 24)] {
        let path = Path::new("/recorder-integer").join(format!("pcm{bits}.wav"));
        let mut take = take(&mut files, &path, encoding, 1);
        take.write(&[-2.0, 2.0], 48_000).unwrap();
        take.write(&[0.0; 2048], 48_000).unwrap();
        assert_eq!(take.finish().unwrap(), 2050);
        let mut reader = files.reader(path);
        assert_eq!(reader.spec().bits_per_sample, bits);
        let samples: Vec<i32> = reader.samples().map(Result::unwrap).collect();
        assert_eq!(&samples[..2], &[-(1 << (bits - 1)), (1 << (bits - 1)) - 1]);
        assert!(samples[2..].contains(&1) && samples[2..].contains(&-1));
        assert!(samples[2..].iter().all(|sample| sample.abs() <= 1));
    }
}
