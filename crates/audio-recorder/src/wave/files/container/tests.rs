use super::WaveWriter;
use hound::{Sample, SampleFormat, WavSpec};
use mixrelay_test_support::{FixtureDir, rf64};
use std::{
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom},
    path::Path,
};

fn spec(channels: u16, bits: u16, format: SampleFormat) -> WavSpec {
    WavSpec {
        channels,
        sample_rate: 48_000,
        bits_per_sample: bits,
        sample_format: format,
    }
}

#[test]
fn every_encoding_is_rf64_with_exact_sizes_counts_padding_and_pcm() {
    let directory = FixtureDir::new("recorder-rf64-formats").unwrap();
    for channels in 1..=mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS {
        for (bits, format) in [
            (16, SampleFormat::Int),
            (24, SampleFormat::Int),
            (32, SampleFormat::Float),
        ] {
            for frames in [0, 1, 3] {
                let path = directory.join(format!("{channels}-{bits}-{frames}.wav"));
                check_format(&path, spec(channels, bits, format), frames);
            }
        }
    }
}

fn check_format(path: &Path, format: WavSpec, frames: u64) {
    let mut writer =
        WaveWriter::new(BufWriter::new(File::create_new(path).unwrap()), format).unwrap();
    for _ in 0..frames * u64::from(format.channels) {
        if format.sample_format == SampleFormat::Float {
            writer.write_sample(0.25_f32).unwrap();
        } else {
            writer.write_sample(-123_i32).unwrap();
        }
    }
    writer.finalize().unwrap();
    let metadata = rf64::inspect(path).unwrap();
    assert_eq!(metadata.frames, frames);
    assert_eq!(
        metadata.data_bytes,
        frames * u64::from(format.channels) * u64::from(format.bits_per_sample / 8)
    );
    let mut reader = rf64::open(path).unwrap();
    assert_eq!(reader.spec(), format);
    assert_eq!(
        reader.len(),
        u32::try_from(frames * u64::from(format.channels)).unwrap()
    );
    if format.sample_format == SampleFormat::Float {
        assert!(
            reader
                .samples::<f32>()
                .all(|sample| sample.unwrap().to_bits() == 0.25_f32.to_bits())
        );
    } else {
        assert!(
            reader
                .samples::<i32>()
                .all(|sample| sample.unwrap() == -123)
        );
    }
    let bytes = std::fs::read(path).unwrap();
    if metadata.data_bytes % 2 == 1 {
        assert_eq!(bytes.last(), Some(&0));
    }
    if format.sample_format == SampleFormat::Float {
        let fact = usize::try_from(metadata.data_offset).unwrap() - 20;
        assert_eq!(&bytes[fact..fact + 4], b"fact");
        assert_eq!(&bytes[fact + 8..fact + 12], &u32::MAX.to_le_bytes());
    }
}

/// Jump through a sparse zero prefix, then write actual PCM across 4 GiB using
/// the production sample writer and finalizer. This checks addressing, not load.
fn sparse_recording(path: &Path, format: WavSpec, frames: u64) {
    let mut writer =
        WaveWriter::new(BufWriter::new(File::create_new(path).unwrap()), format).unwrap();
    let frame_bytes = u64::from(format.channels) * u64::from(format.bits_per_sample / 8);
    let prefix_frames = frames - 256;
    writer.data_bytes = prefix_frames * frame_bytes;
    let tail_offset = writer.header.len() as u64 + writer.data_bytes;
    writer.stream.seek(SeekFrom::Start(tail_offset)).unwrap();
    let mut expected = Vec::new();
    for sample in 0..256 * i32::from(format.channels) {
        if format.sample_format == SampleFormat::Float {
            let value = (f32::from(i16::try_from(sample).unwrap()) + 1.0) / 4096.0;
            writer.write_sample(value).unwrap();
            value.write(&mut expected, 32).unwrap();
        } else {
            writer.write_sample(sample - 123).unwrap();
            (sample - 123)
                .write(&mut expected, format.bits_per_sample)
                .unwrap();
        }
    }
    writer.finalize().unwrap();
    let metadata = rf64::inspect(path).unwrap();
    assert_eq!(metadata.data_bytes, frames * frame_bytes);
    assert_eq!(
        metadata.riff_bytes,
        metadata.data_offset - 8 + metadata.data_bytes + metadata.data_bytes % 2
    );
    assert_eq!(metadata.frames, frames);
    let mut file = File::open(path).unwrap();
    file.seek(SeekFrom::Start(tail_offset)).unwrap();
    let mut actual = vec![0; expected.len()];
    file.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    decode_sparse_tail(path, &metadata, tail_offset, &expected);
}

#[test]
fn every_format_decodes_around_the_4_gib_boundary_without_rollover() {
    let directory = FixtureDir::new("rec-rf64-large").unwrap();
    for channels in 1..=mixrelay_audio_io::formats::MAX_RECORDING_CHANNELS {
        for (bits, format) in [
            (16, SampleFormat::Int),
            (24, SampleFormat::Int),
            (32, SampleFormat::Float),
        ] {
            let frame_bytes = u64::from(channels) * u64::from(bits / 8);
            let boundary = (1_u64 << 32) / frame_bytes;
            // Nearest complete frames below, at (where representable), and above 4 GiB.
            for frames in [boundary - 1, boundary, boundary + 1] {
                let path = directory.join(format!("{channels}-{bits}-{frames}.wav"));
                sparse_recording(&path, spec(channels, bits, format), frames);
            }
        }
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 72);
}

#[test]
fn unfinished_frame_and_u64_exhaustion_fail_without_silent_size_wrap() {
    let directory = FixtureDir::new("recorder-rf64-invalid").unwrap();
    let mut writer = WaveWriter::new(
        BufWriter::new(File::create_new(directory.join("partial.wav")).unwrap()),
        spec(2, 16, SampleFormat::Int),
    )
    .unwrap();
    writer.write_sample(12_i32).unwrap();
    assert!(
        writer
            .finalize()
            .unwrap_err()
            .to_string()
            .contains("unfinished")
    );
    let mut writer = WaveWriter::new(
        std::io::Cursor::new(Vec::new()),
        spec(1, 32, SampleFormat::Float),
    )
    .unwrap();
    writer.data_bytes = u64::MAX;
    assert!(writer.write_sample(0.25_f32).is_err());
    assert!(writer.finalize().is_err());
}

// Hound is RIFF-only. Preserve the production fmt/fact chunks and translate only
// RF64 sizes while exposing a bounded data window beyond 4 GiB to its decoder.
fn decode_sparse_tail(path: &Path, metadata: &rf64::Metadata, offset: u64, expected: &[u8]) {
    let mut file = File::open(path).unwrap();
    let mut header = vec![0; usize::try_from(metadata.data_offset).unwrap()];
    file.read_exact(&mut header).unwrap();
    header[..4].copy_from_slice(b"RIFF");
    let size = u32::try_from(header.len() - 8 + expected.len()).unwrap();
    header[4..8].copy_from_slice(&size.to_le_bytes());
    header[12..16].copy_from_slice(b"JUNK");
    let data_size = header.len() - 4;
    header[data_size..].copy_from_slice(&u32::try_from(expected.len()).unwrap().to_le_bytes());
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut actual = vec![0; expected.len()];
    file.read_exact(&mut actual).unwrap();
    header.extend_from_slice(&actual);
    let mut reader = hound::WavReader::new(std::io::Cursor::new(header)).unwrap();
    let format = reader.spec();
    assert_eq!(reader.duration(), 256);
    let mut decoded = Vec::new();
    if format.sample_format == SampleFormat::Float {
        for sample in reader.samples::<f32>() {
            sample
                .unwrap()
                .write(&mut decoded, format.bits_per_sample)
                .unwrap();
        }
    } else {
        for sample in reader.samples::<i32>() {
            sample
                .unwrap()
                .write(&mut decoded, format.bits_per_sample)
                .unwrap();
        }
    }
    assert_eq!(decoded, expected);
}
