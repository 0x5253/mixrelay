use crate::recording_files::FileFactory;
use crate::{Encoding, RecorderRequest, transport::PACKET_FRAMES};
use crate::{RecorderError, RecorderFailure};
use audioadapter_buffers::direct::InterleavedSlice;
use hound::{SampleFormat, WavSpec};
use rubato::{Async, FixedAsync, Resampler, SincInterpolationParameters};
use std::path::Path;

mod files;
use files::WaveFile;

pub(crate) struct Take<F: FileFactory> {
    sink: WaveSink<F>,
    segment: Option<Segment>,
    // Fractional output-frame carry prevents rounding each rate segment from
    // changing the total recording duration after repeated rate transitions.
    fraction: f64,
}

impl<F: FileFactory> Take<F> {
    pub(crate) fn new(
        request: &RecorderRequest,
        channels: u16,
        observed: u32,
        files: &mut F,
    ) -> Result<Self, RecorderError> {
        if !crate::valid_rate(observed) {
            return Err(RecorderFailure::InputRateUnobserved.into());
        }
        let path = request
            .path
            .as_deref()
            .ok_or(RecorderFailure::MissingPath)?;
        if !Path::new(path).is_absolute() {
            return Err(RecorderFailure::RelativePath.into());
        }
        let rate = request.sample_rate.unwrap_or(observed);
        if !crate::valid_rate(rate) {
            return Err(RecorderFailure::UnsupportedRate.into());
        }
        let encoding = request.encoding.unwrap_or(Encoding::Pcm24);
        let spec = WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: match encoding {
                Encoding::Pcm16 => 16,
                Encoding::Pcm24 => 24,
                Encoding::Float32 => 32,
            },
            sample_format: if encoding == Encoding::Float32 {
                SampleFormat::Float
            } else {
                SampleFormat::Int
            },
        };
        // Build the first converter before Record opens callback admission.
        // Packet rate tags still replace it if the actual clock changes here.
        let segment = Segment::new(observed, rate, usize::from(channels))?;
        let file = WaveFile::new(Path::new(path), spec, files)?;
        Ok(Self {
            sink: WaveSink {
                file,
                rate,
                encoding,
                channels: usize::from(channels),
                frames: 0,
                noise: Noise(0x8c67_24fb_69da_b813),
            },
            segment: Some(segment),
            fraction: 0.0,
        })
    }

    pub(crate) fn frames(&self) -> u64 {
        self.sink.frames
    }
    pub(crate) fn rate(&self) -> u32 {
        self.sink.rate
    }
    pub(crate) fn encoding(&self) -> Encoding {
        self.sink.encoding
    }

    pub(crate) fn write(&mut self, samples: &[f32], rate: u32) -> Result<(), RecorderError> {
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(RecorderFailure::NonFinitePcm.into());
        }
        self.prepare_rate(rate)?;
        let Some(segment) = self.segment.as_mut() else {
            return Err(RecorderFailure::SegmentUnprepared.into());
        };
        segment.push(samples, &mut self.sink)
    }

    pub(crate) fn prepare_rate(&mut self, rate: u32) -> Result<(), RecorderError> {
        if !crate::valid_rate(rate) {
            return Err(RecorderFailure::InputRateUnobserved.into());
        }
        if self
            .segment
            .as_ref()
            .is_none_or(|segment| segment.rate != rate)
        {
            self.flush_segment()?;
            self.segment = Some(Segment::new(rate, self.sink.rate, self.sink.channels)?);
        }
        Ok(())
    }

    pub(crate) fn flush_segment(&mut self) -> Result<(), RecorderError> {
        if let Some(mut segment) = self.segment.take() {
            segment.finish(&mut self.sink, &mut self.fraction)?;
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<u64, RecorderError> {
        let conversion = self.flush_segment();
        let finalization = self.sink.finalize();
        match (conversion, finalization) {
            (Ok(()), result) => result,
            (Err(error), Ok(_)) => Err(error),
            (Err(error), Err(finalization)) if error.same_failure(&finalization) => Err(error),
            (Err(error), Err(finalization)) => Err(error.combine(finalization)),
        }
    }
}

struct WaveSink<F: FileFactory> {
    file: WaveFile<F>,
    channels: usize,
    rate: u32,
    encoding: Encoding,
    frames: u64,
    noise: Noise,
}

impl<F: FileFactory> WaveSink<F> {
    fn write(&mut self, samples: &[f32]) -> Result<(), RecorderError> {
        for frame in samples.chunks_exact(self.channels) {
            let next = self
                .frames
                .checked_add(1)
                .ok_or(RecorderFailure::FrameOverflow)?;
            for sample in frame {
                let result = match self.encoding {
                    Encoding::Float32 => self.file.write_sample(*sample),
                    Encoding::Pcm16 => self
                        .file
                        .write_sample(self.noise.quantize(*sample, 32_768.0)),
                    Encoding::Pcm24 => self
                        .file
                        .write_sample(self.noise.quantize(*sample, 8_388_608.0)),
                };
                result?;
            }
            self.frames = next;
        }
        Ok(())
    }

    fn finalize(mut self) -> Result<u64, RecorderError> {
        self.file.finalize()?;
        Ok(self.frames)
    }
}

#[cfg(test)]
mod tests;

struct Noise(u64);
impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        f64::from((self.0 >> 32) as u32) / 4_294_967_296.0
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "rounded value is clamped to signed PCM16/24 range"
    )]
    fn quantize(&mut self, value: f32, scale: f64) -> i32 {
        let dither = self.uniform() - self.uniform();
        (f64::from(value).mul_add(scale, dither))
            .round()
            .clamp(-scale, scale - 1.0) as i32
    }
}

struct Segment {
    rate: u32,
    output_rate: u32,
    channels: usize,
    resampler: Option<Async<f32>>,
    input: Vec<f32>,
    output: Vec<f32>,
    buffered: usize,
    input_frames: u64,
    emitted: u64,
    skip: usize,
}

impl Segment {
    fn new(rate: u32, output_rate: u32, channels: usize) -> Result<Self, RecorderError> {
        let resampler = if rate == output_rate {
            None
        } else {
            Some(
                Async::new_sinc(
                    f64::from(output_rate) / f64::from(rate),
                    1.0,
                    &SincInterpolationParameters::default(),
                    PACKET_FRAMES,
                    channels,
                    FixedAsync::Input,
                )
                .map_err(RecorderFailure::ConverterPreparation)?,
            )
        };
        let capacity = resampler.as_ref().map_or(0, Resampler::output_frames_max);
        let skip = resampler.as_ref().map_or(0, Resampler::output_delay);
        Ok(Self {
            rate,
            output_rate,
            channels,
            resampler,
            input: mixrelay_audio_io::pcm_buffer::zeroed(PACKET_FRAMES * channels)?,
            output: mixrelay_audio_io::pcm_buffer::zeroed(capacity * channels)?,
            buffered: 0,
            input_frames: 0,
            emitted: 0,
            skip,
        })
    }

    fn push<F: FileFactory>(
        &mut self,
        mut samples: &[f32],
        sink: &mut WaveSink<F>,
    ) -> Result<(), RecorderError> {
        self.input_frames = self
            .input_frames
            .checked_add(
                u64::try_from(samples.len() / self.channels)
                    .map_err(|_| RecorderFailure::InputLengthOverflow)?,
            )
            .ok_or(RecorderFailure::DurationOverflow)?;
        if self.resampler.is_none() {
            return sink.write(samples);
        }
        while !samples.is_empty() {
            let count = samples.len().min(self.input.len() - self.buffered);
            self.input[self.buffered..self.buffered + count].copy_from_slice(&samples[..count]);
            self.buffered += count;
            samples = &samples[count..];
            if self.buffered == self.input.len() {
                self.process(sink, None)?;
            }
        }
        Ok(())
    }

    fn process<F: FileFactory>(
        &mut self,
        sink: &mut WaveSink<F>,
        target: Option<u64>,
    ) -> Result<(), RecorderError> {
        let input = InterleavedSlice::new(&self.input, self.channels, PACKET_FRAMES)
            .map_err(RecorderFailure::from)?;
        let capacity = self.output.len() / self.channels;
        let mut output = InterleavedSlice::new_mut(&mut self.output, self.channels, capacity)
            .map_err(RecorderFailure::from)?;
        let (_, produced) = self
            .resampler
            .as_mut()
            .ok_or(RecorderFailure::ConverterMissing)?
            .process_into_buffer(&input, &mut output, None)
            .map_err(RecorderFailure::from)?;
        let skip = self.skip.min(produced);
        self.skip -= skip;
        let count = target.map_or(produced - skip, |limit| {
            (produced - skip)
                .min(usize::try_from(limit.saturating_sub(self.emitted)).unwrap_or(usize::MAX))
        });
        sink.write(&self.output[skip * self.channels..(skip + count) * self.channels])?;
        self.emitted += u64::try_from(count).map_err(|_| RecorderFailure::OutputLengthOverflow)?;
        self.buffered = 0;
        self.input.fill(0.0);
        Ok(())
    }

    fn finish<F: FileFactory>(
        &mut self,
        sink: &mut WaveSink<F>,
        fraction: &mut f64,
    ) -> Result<(), RecorderError> {
        if self.resampler.is_none() {
            return Ok(());
        }
        let numerator = u128::from(self.input_frames) * u128::from(self.output_rate);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a remainder modulo a u32 rate is below that rate"
        )]
        let remainder = (numerator % u128::from(self.rate)) as u32;
        let partial = *fraction + f64::from(remainder) / f64::from(self.rate);
        let extra = u64::from(partial >= 0.5);
        *fraction = partial - if extra == 1 { 1.0 } else { 0.0 };
        let target = u64::try_from(numerator / u128::from(self.rate))
            .map_err(|_| RecorderFailure::DurationOverflow)?
            .checked_add(extra)
            .ok_or(RecorderFailure::DurationOverflow)?;
        if self.emitted > target {
            return Err(RecorderFailure::ConversionExceededDuration.into());
        }
        for _ in 0..128 {
            if self.emitted == target {
                return Ok(());
            }
            self.process(sink, Some(target))?;
        }
        Err(RecorderFailure::ConversionTailIncomplete.into())
    }
}
