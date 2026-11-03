//! RF64 container sizes are 64-bit here. Hound still owns format headers
//! and sample encoding; its `WavWriter`'s 32-bit byte counter is never used for PCM.
use hound::{Sample, SampleFormat, WavSpec, WavWriter};
use std::io::{self, Cursor, SeekFrom};

use crate::recording_files::RecordingStream;

pub(super) struct WaveWriter<W: RecordingStream> {
    stream: W,
    spec: WavSpec,
    header: Vec<u8>,
    data_bytes: u64,
}

impl<W: RecordingStream> WaveWriter<W> {
    pub(super) fn new(stream: W, spec: WavSpec) -> Result<Self, hound::Error> {
        // Reuse Hound's validated fmt chunk, including WAVE_FORMAT_EXTENSIBLE
        // for multichannel and 24-bit PCM. Only the container size fields change.
        let mut original = Vec::new();
        WavWriter::new(Cursor::new(&mut original), spec)?.finalize()?;
        let mut header = original[..12].to_vec();
        header.extend_from_slice(b"ds64");
        header.extend_from_slice(&28_u32.to_le_bytes());
        header.extend_from_slice(&[0; 28]);
        header.extend_from_slice(&original[12..original.len() - 8]);
        if spec.sample_format == SampleFormat::Float {
            header.extend_from_slice(b"fact");
            header.extend_from_slice(&4_u32.to_le_bytes());
            header.extend_from_slice(&0_u32.to_le_bytes());
        }
        header.extend_from_slice(&original[original.len() - 8..]);
        let mut writer = Self {
            stream,
            spec,
            header,
            data_bytes: 0,
        };
        writer.update_header()?;
        writer
            .stream
            .seek(SeekFrom::Start(writer.header.len() as u64))?;
        Ok(writer)
    }

    fn riff_size(&self, data_bytes: u64) -> io::Result<u64> {
        (self.header.len() as u64 - 8)
            .checked_add(data_bytes)
            .and_then(|size| size.checked_add(data_bytes % 2))
            .ok_or_else(|| io::Error::other("WAV size exhausted"))
    }

    pub(super) fn write_sample<S: Sample>(&mut self, sample: S) -> Result<(), hound::Error> {
        let next = self
            .data_bytes
            .checked_add(u64::from(self.spec.bits_per_sample / 8))
            .ok_or_else(|| io::Error::other("WAV size exhausted"))?;
        sample.write(&mut self.stream, self.spec.bits_per_sample)?;
        self.data_bytes = next;
        Ok(())
    }

    fn update_header(&mut self) -> io::Result<()> {
        let riff_size = self.riff_size(self.data_bytes)?;
        let frame_bytes = u64::from(self.spec.channels) * u64::from(self.spec.bits_per_sample / 8);
        let frames = self.data_bytes / frame_bytes;
        let data_offset = self.header.len() - 4;
        self.header[..4].copy_from_slice(b"RF64");
        self.header[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        self.header[12..16].copy_from_slice(b"ds64");
        self.header[20..28].copy_from_slice(&riff_size.to_le_bytes());
        self.header[28..36].copy_from_slice(&self.data_bytes.to_le_bytes());
        self.header[36..44].copy_from_slice(&frames.to_le_bytes());
        self.header[data_offset..].copy_from_slice(&u32::MAX.to_le_bytes());
        if self.spec.sample_format == SampleFormat::Float {
            let fact_offset = self.header.len() - 12;
            let count = u32::MAX;
            self.header[fact_offset..fact_offset + 4].copy_from_slice(&count.to_le_bytes());
        }
        self.stream.seek(SeekFrom::Start(0))?;
        self.stream.write_all(&self.header)
    }

    pub(super) fn finalize(mut self) -> io::Result<()> {
        let frame_bytes = u64::from(self.spec.channels) * u64::from(self.spec.bits_per_sample / 8);
        let incomplete_frame = !self.data_bytes.is_multiple_of(frame_bytes);
        self.data_bytes -= self.data_bytes % frame_bytes;
        let end = (self.header.len() as u64)
            .checked_add(self.data_bytes)
            .ok_or_else(|| io::Error::other("WAV size exhausted"))?;
        // A failed write can leave either a partial sample or some channels of
        // the final frame. Only whole frames belong to the recoverable prefix.
        self.stream.truncate(end)?;
        self.stream.seek(SeekFrom::Start(end))?;
        if !self.data_bytes.is_multiple_of(2) {
            self.stream.write_all(&[0])?;
        }
        self.update_header()?;
        self.stream.flush()?;
        if incomplete_frame {
            return Err(io::Error::other("unfinished channel frame"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
