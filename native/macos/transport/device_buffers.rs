//! Validated native stream selection and PCM projection for a device callback.
use std::{ops::Range, slice};

use mixrelay_pcm_ring::SampleFormat;

use crate::audio_buffer::{self, AudioBuffer, AudioBufferList};

/// Immutable selected device slice, resolved before registering the callback.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BufferSelection {
    /// Number of selected graph channels (1..64).
    pub channels: u32,
    /// First selected native channel across all streams.
    pub offset: u32,
    /// Exact aggregate width, or zero for an ordinary device.
    pub total: u32,
    /// Native device width limit: 64, or 2048 for an owned aggregate.
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BufferFormat {
    channels: u32,
    pcm: SampleFormat,
}

/// Control-prepared ASBD-to-buffer mapping, including each planar channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeFormats {
    values: [BufferFormat; 2048],
    count: usize,
    float_only: bool,
}

impl Default for NativeFormats {
    fn default() -> Self {
        Self {
            values: [BufferFormat {
                channels: 0,
                pcm: SampleFormat::Float32,
            }; 2048],
            count: 0,
            float_only: true,
        }
    }
}

impl NativeFormats {
    /// Appends one validated ASBD stream; planar streams map to one buffer per channel.
    /// Returns false without changing the mapping when its fixed capacity is exhausted.
    pub(crate) fn append(&mut self, channels: u32, planar: bool, pcm: SampleFormat) -> bool {
        let count = if planar { channels as usize } else { 1 };
        let Some(end) = self
            .count
            .checked_add(count)
            .filter(|&end| end <= self.values.len())
        else {
            return false;
        };
        self.values[self.count..end].fill(BufferFormat {
            channels: if planar { 1 } else { channels },
            pcm,
        });
        self.count = end;
        self.float_only &= pcm == SampleFormat::Float32;
        true
    }

    /// Whether the complete admitted native layout can use the existing F32 direct ABI.
    pub(crate) fn all_float(&self) -> bool {
        self.count != 0 && self.float_only
    }
}

struct Layout<'a> {
    buffers: &'a [AudioBuffer],
    selection: BufferSelection,
    frames: usize,
    formats: Option<&'a [BufferFormat]>,
}

impl<'a> Layout<'a> {
    fn validate(buffers: &'a [AudioBuffer], selection: BufferSelection) -> Option<Self> {
        Self::validate_formats(buffers, selection, None)
    }

    fn validate_native(
        buffers: &'a [AudioBuffer],
        selection: BufferSelection,
        formats: &'a NativeFormats,
    ) -> Option<Self> {
        if formats.count != buffers.len() {
            return None;
        }
        Self::validate_formats(buffers, selection, Some(&formats.values[..formats.count]))
    }

    fn validate_formats(
        buffers: &'a [AudioBuffer],
        selection: BufferSelection,
        formats: Option<&'a [BufferFormat]>,
    ) -> Option<Self> {
        let end = selection.offset.checked_add(selection.channels)?;
        if !(1..=64).contains(&selection.channels)
            || !matches!(selection.limit, 64 | 2048)
            || end > selection.limit
        {
            return None;
        }
        let mut width = 0_u32;
        let mut frames = 0;
        for (index, buffer) in buffers.iter().enumerate() {
            if buffer.channels == 0 || buffer.channels > selection.limit {
                return None;
            }
            let format = formats.map_or(SampleFormat::Float32, |formats| formats[index].pcm);
            if formats.is_some_and(|formats| formats[index].channels != buffer.channels) {
                return None;
            }
            let sample_bytes = u32::try_from(format.bytes_per_sample()).ok()?;
            let stride = buffer.channels * sample_bytes;
            if !buffer.byte_size.is_multiple_of(stride) {
                return None;
            }
            let next = width.checked_add(buffer.channels)?;
            if buffer.data.is_null() {
                if width < end && next > selection.offset {
                    return None;
                }
            } else {
                let count = buffer.byte_size / stride;
                let alignment = if format == SampleFormat::Signed24 {
                    1
                } else {
                    sample_bytes as usize
                };
                if !buffer.data.addr().is_multiple_of(alignment)
                    || !(1..=4096).contains(&count)
                    || (frames != 0 && frames != count)
                {
                    return None;
                }
                frames = count;
            }
            width = next;
        }
        if frames == 0
            || width < end
            || width > selection.limit
            || (selection.total != 0 && width != selection.total)
        {
            return None;
        }
        Some(Self {
            buffers,
            selection,
            frames: frames as usize,
            formats,
        })
    }

    fn segments(&self) -> impl Iterator<Item = (&AudioBuffer, Range<usize>, usize, SampleFormat)> {
        let selected = self.selection.offset..self.selection.offset + self.selection.channels;
        let mut offset = 0;
        self.buffers
            .iter()
            .enumerate()
            .filter_map(move |(index, buffer)| {
                let start = offset;
                offset += buffer.channels;
                let first = start.max(selected.start);
                let end = offset.min(selected.end);
                (first < end).then(|| {
                    (
                        buffer,
                        (first - start) as usize..(end - start) as usize,
                        (first - selected.start) as usize,
                        self.formats
                            .map_or(SampleFormat::Float32, |formats| formats[index].pcm),
                    )
                })
            })
    }

    // SAFETY: native selected PCM and scratch are complete, live and disjoint;
    // selected source storage is immutable for this callback.
    unsafe fn capture(&self, scratch: *mut f32) {
        // SAFETY: callback owns exactly frames*selected channels in scratch.
        let target = unsafe {
            slice::from_raw_parts_mut(scratch, self.frames * self.selection.channels as usize)
        };
        for (buffer, range, selected, format) in self.segments() {
            if format != SampleFormat::Float32 {
                // SAFETY: format-specific byte extent/alignment and selected projection were
                // validated before any transfer; this callback owns disjoint scratch.
                unsafe {
                    capture_pcm(
                        (buffer, range, selected, format),
                        target,
                        self.selection.channels as usize,
                    );
                };
                continue;
            }
            // SAFETY: validation proves selected stream is nonnull, aligned and
            // holds this many f32 samples; native callback owns the actual extent.
            let source = unsafe {
                slice::from_raw_parts(buffer.data, self.frames * buffer.channels as usize)
            };
            if source.len() == target.len() && range.len() == self.selection.channels as usize {
                target.copy_from_slice(source);
                return;
            }
            if buffer.channels == 1 {
                // A mono plane needs one scalar store per frame, not a dynamic
                // memcpy per sample. Keep this distinct from mixed stream spans.
                for (&sample, graph) in source
                    .iter()
                    .zip(target.chunks_exact_mut(self.selection.channels as usize))
                {
                    graph[selected] = sample;
                }
                continue;
            }
            for (native, graph) in source
                .chunks_exact(buffer.channels as usize)
                .zip(target.chunks_exact_mut(self.selection.channels as usize))
            {
                graph[selected..selected + range.len()].copy_from_slice(&native[range.clone()]);
            }
        }
    }

    // SAFETY: callback exclusively owns selected output PCM and scratch is a
    // disjoint immutable input. Per-stream mutable borrows do not overlap in time.
    unsafe fn render(&self, scratch: *const f32) {
        // SAFETY: native callback retains a complete initialized scratch block.
        let source = unsafe {
            slice::from_raw_parts(scratch, self.frames * self.selection.channels as usize)
        };
        for (buffer, range, selected, format) in self.segments() {
            if format != SampleFormat::Float32 {
                // SAFETY: native writable byte extent and disjoint graph input are validated.
                unsafe {
                    render_pcm(
                        (buffer, range, selected, format),
                        source,
                        self.selection.channels as usize,
                    );
                };
                continue;
            }
            // SAFETY: selected backing PCM was validated and this callback owns
            // its output; the borrow ends before processing another stream.
            let target = unsafe {
                slice::from_raw_parts_mut(buffer.data, self.frames * buffer.channels as usize)
            };
            if target.len() == source.len() && range.len() == self.selection.channels as usize {
                target.copy_from_slice(source);
                return;
            }
            if buffer.channels == 1 {
                for (sample, graph) in target
                    .iter_mut()
                    .zip(source.chunks_exact(self.selection.channels as usize))
                {
                    *sample = graph[selected];
                }
                continue;
            }
            for (native, graph) in target
                .chunks_exact_mut(buffer.channels as usize)
                .zip(source.chunks_exact(self.selection.channels as usize))
            {
                native[range.clone()].copy_from_slice(&graph[selected..selected + range.len()]);
            }
        }
    }
}

// SAFETY: Layout admits exact native extents; byte views never form integer/f32 references.
unsafe fn capture_pcm(
    segment: (&AudioBuffer, Range<usize>, usize, SampleFormat),
    target: &mut [f32],
    channels: usize,
) {
    let (buffer, range, selected, format) = segment;
    let bytes = format.bytes_per_sample();
    // SAFETY: borrowed complete initialized native PCM, retained throughout this callback.
    let source =
        unsafe { slice::from_raw_parts(buffer.data.cast::<u8>(), buffer.byte_size as usize) };
    for (native, graph) in source
        .chunks_exact(buffer.channels as usize * bytes)
        .zip(target.chunks_exact_mut(channels))
    {
        let _ = format.decode(
            &native[range.start * bytes..range.end * bytes],
            &mut graph[selected..selected + range.len()],
        );
    }
}

// SAFETY: Layout admits writable extents; only the selected native channel bytes are changed.
unsafe fn render_pcm(
    segment: (&AudioBuffer, Range<usize>, usize, SampleFormat),
    source: &[f32],
    channels: usize,
) {
    let (buffer, range, selected, format) = segment;
    let bytes = format.bytes_per_sample();
    // SAFETY: callback exclusively owns native output, disjoint from graph scratch.
    let target =
        unsafe { slice::from_raw_parts_mut(buffer.data.cast::<u8>(), buffer.byte_size as usize) };
    for (native, graph) in target
        .chunks_exact_mut(buffer.channels as usize * bytes)
        .zip(source.chunks_exact(channels))
    {
        let _ = format.encode(
            &graph[selected..selected + range.len()],
            &mut native[range.start * bytes..range.end * bytes],
        );
    }
}

/// Reads frame geometry using the control-admitted native codec mapping.
/// # Safety
/// Native metadata and PCM extents remain retained for the callback.
pub(crate) unsafe fn native_frames(
    list: *const AudioBufferList,
    selection: BufferSelection,
    formats: &NativeFormats,
) -> u32 {
    // SAFETY: complete native metadata; no PCM access before full layout validation.
    unsafe { audio_buffer::buffers(list, selection.limit as usize) }
        .and_then(|buffers| Layout::validate_native(buffers, selection, formats))
        .and_then(|layout| u32::try_from(layout.frames).ok())
        .unwrap_or(0)
}

/// Transfers admitted native PCM through preallocated F32 scratch.
/// # Safety
/// Same native extents/disjoint scratch ownership as the public F32 transfer ABI.
pub(crate) unsafe fn native_transfer(
    list: *const AudioBufferList,
    selection: BufferSelection,
    formats: &NativeFormats,
    scratch: *mut f32,
    input: bool,
) -> bool {
    if scratch.is_null() || !scratch.is_aligned() {
        return false;
    }
    // SAFETY: metadata is callback-owned; validation precedes all PCM borrows.
    let layout = unsafe { audio_buffer::buffers(list, selection.limit as usize) }
        .and_then(|buffers| Layout::validate_native(buffers, selection, formats));
    let Some(layout) = layout else {
        return false;
    };
    // SAFETY: all selected extents were admitted and scratch is disjoint and full-sized.
    unsafe {
        if input {
            layout.capture(scratch);
        } else {
            layout.render(scratch);
        }
    }
    true
}

/// Validates the complete native layout; returns its frame count or zero.
/// # Safety
/// List owns its complete native trailing metadata array for this call. This
/// function inspects metadata only; PCM must remain live for later transfer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_buffers_frames(
    list: *const AudioBufferList,
    selection: BufferSelection,
) -> u32 {
    // SAFETY: retained callback metadata, bounded by the selected device limit.
    unsafe { audio_buffer::buffers(list, selection.limit as usize) }
        .and_then(|buffers| Layout::validate(buffers, selection))
        .and_then(|layout| u32::try_from(layout.frames).ok())
        .unwrap_or(0)
}

/// Transfers selected native channels to/from interleaved scratch. Invalid layout
/// returns false before copying; does not own, allocate or release storage.
/// # Safety
/// Native list/selected PCM remain live and stable for this call. Scratch is
/// aligned, initialized and sized for frames*selection.channels floats, disjoint
/// from native buffers. Capture owns scratch exclusively; render owns native PCM.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_buffers_transfer(
    list: *const AudioBufferList,
    selection: BufferSelection,
    scratch: *mut f32,
    input: bool,
) -> bool {
    if scratch.is_null() || !scratch.is_aligned() {
        return false;
    }
    // SAFETY: retained native callback metadata; validation precedes PCM borrows.
    let layout = unsafe { audio_buffer::buffers(list, selection.limit as usize) }
        .and_then(|buffers| Layout::validate(buffers, selection));
    let Some(layout) = layout else {
        return false;
    };
    // SAFETY: complete selected PCM spans and disjoint scratch are callback-owned.
    unsafe {
        if input {
            layout.capture(scratch);
        } else {
            layout.render(scratch);
        }
    }
    true
}

#[cfg(test)]
mod tests;
