//! Off-callback WAV export for the installed-route measurement only.
use super::{CaptureEvidence, Result, require};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};

pub(super) fn save(evidence: &CaptureEvidence, rate: u32) -> Result<()> {
    let Some(prefix) = std::env::var_os("MIXRELAY_CAPTURE_OUTPUT") else {
        return Ok(());
    };
    let prefix = PathBuf::from(prefix);
    require(prefix.is_absolute(), "recording prefix must be absolute")?;
    for &channel in &evidence.selected {
        let data = &evidence.channels[usize::from(channel)];
        let count = usize::try_from(data.samples.load(Ordering::Relaxed))?;
        require(
            count <= data.pcm.len(),
            "recording exceeded preallocated PCM capacity",
        )?;
        let path = prefix.with_extension(format!("ch{}.wav", channel + 1));
        let mut file = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?,
        );
        write_pcm(&mut file, &data.pcm[..count], rate)?;
        file.flush()?;
        println!(
            "captured_pcm path={} frames={count} rate={rate} first_host_time={}",
            path.display(),
            evidence.first_host_time.load(Ordering::Relaxed)
        );
    }
    Ok(())
}

fn write_pcm(file: &mut impl Write, samples: &[AtomicU32], rate: u32) -> Result<()> {
    let bytes = u32::try_from(samples.len().checked_mul(4).ok_or("PCM size overflow")?)?;
    let riff_size = bytes.checked_add(36).ok_or("RIFF size overflow")?;
    let byte_rate = rate.checked_mul(4).ok_or("PCM rate overflow")?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVEfmt \x10\0\0\0\x03\0\x01\0")?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(b"\x04\0\x20\0data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for sample in samples {
        file.write_all(&sample.load(Ordering::Relaxed).to_le_bytes())?;
    }
    Ok(())
}

#[test]
fn float_wav_keeps_sample_bits_and_complete_header() {
    let samples = [-0.25_f32, 0.0, 0.75].map(|sample| AtomicU32::new(sample.to_bits()));
    let mut bytes = Vec::new();
    write_pcm(&mut bytes, &samples, 48_000).unwrap();
    assert_eq!(bytes.len(), 56);
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[4..8], &48_u32.to_le_bytes());
    assert_eq!(&bytes[20..24], b"\x03\0\x01\0");
    assert_eq!(&bytes[24..28], &48_000_u32.to_le_bytes());
    assert_eq!(&bytes[36..40], b"data");
    assert_eq!(&bytes[40..44], &12_u32.to_le_bytes());
    for (actual, expected) in bytes[44..].chunks_exact(4).zip(samples) {
        assert_eq!(actual, expected.load(Ordering::Relaxed).to_le_bytes());
    }
}
