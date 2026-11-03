use super::Binding;
use mixrelay_macos_transport::audio_buffer::{self, AudioBuffer, AudioBufferList};
use std::{ffi::c_void, marker::PhantomData, ops::Range, ptr, slice};

/// Callback-scoped access to native PCM. Only descriptor references are retained;
/// each operation borrows one PCM extent and releases it before the next extent.
/// This permits native in-place and overlapping stream layouts without ever
/// creating simultaneous shared/exclusive Rust references to their PCM.
pub(super) struct Buffers<'a> {
    input: &'a [AudioBuffer],
    output: &'a [AudioBuffer],
    frames: usize,
    callback: PhantomData<&'a mut f32>,
}

impl Buffers<'_> {
    /// Runs an operation with native PCM access that cannot escape the callback.
    ///
    /// # Safety
    /// Each non-null, aligned list pointer must retain a readable count field. If
    /// that count is 1..=2048, the complete trailing metadata array must also remain
    /// readable and immutable. Metadata rejected by alignment/count/geometry checks
    /// does not require PCM access. For each non-null PCM extent that passes those
    /// checks, its advertised allocation must remain live until the operation ends.
    /// Metadata must not overlap PCM; output PCM is exclusively callback-owned and
    /// input PCM is not concurrently mutated. Engine-owned DSP scratch is disjoint
    /// from native PCM, and the operation must not access native PCM independently
    /// of this view. These are the serialized `DeviceIo`/`RenderSlot` owner contracts.
    /// Input/output PCM may overlap: methods never retain PCM slices between calls.
    pub(super) unsafe fn with_native<R>(
        input: *const c_void,
        output: *mut c_void,
        frames: u32,
        operation: impl for<'callback> FnOnce(Buffers<'callback>) -> R,
    ) -> Option<R> {
        if !(1..=4096).contains(&frames) {
            return None;
        }
        // SAFETY: DeviceIo retains the full, immutable native descriptor arrays;
        // list validates their geometry but never creates PCM references.
        let (input, output) = unsafe { (list(input, frames)?, list(output, frames)?) };
        Some(operation(Buffers {
            input,
            output,
            frames: frames as usize,
            callback: PhantomData,
        }))
    }

    #[cfg(test)]
    pub(super) fn empty() -> Self {
        Self {
            input: &[],
            output: &[],
            frames: 0,
            callback: PhantomData,
        }
    }

    pub(super) fn covers(&self, binding: &Binding) -> bool {
        if binding.offset.is_none() {
            return true;
        }
        let Some(selected) = channels(binding) else {
            return false;
        };
        let buffers = if binding.input {
            self.input
        } else {
            self.output
        };
        let mut base = 0;
        for buffer in buffers {
            let next = base + buffer.channels as usize;
            if base < selected.end && next > selected.start && buffer.data.is_null() {
                return false;
            }
            base = next;
        }
        selected.end <= base
    }

    pub(super) fn transfer(&self, map: &Binding, offset: usize, destination: &mut [f32]) {
        let Some(selection) = self.selection(map, offset, destination.len()) else {
            return;
        };
        let mut base = 0;
        for buffer in self.input {
            let width = buffer.channels as usize;
            if let Some(span) = selection
                .span(base, width)
                .filter(|_| !buffer.data.is_null())
            {
                // SAFETY: with_native retains native PCM; list checked f32 alignment
                // and exact frame extent. This one shared borrow ends in this branch,
                // before any write operation, and DSP scratch is a distinct allocation.
                let pcm = unsafe { slice::from_raw_parts(buffer.data, self.frames * width) };
                copy_input(pcm, destination, &selection, span, width);
            }
            base += width;
        }
    }

    pub(super) fn write(&mut self, map: &Binding, offset: usize, source: &[f32]) {
        let Some(selection) = self.selection(map, offset, source.len()) else {
            return;
        };
        let mut base = 0;
        for buffer in self.output {
            let width = buffer.channels as usize;
            if let Some(span) = selection
                .span(base, width)
                .filter(|_| !buffer.data.is_null())
            {
                // SAFETY: the callback owns writable native PCM and source is separate
                // engine scratch. Only this buffer is borrowed mutably, and its borrow
                // ends before the next descriptor (which may overlap it) is visited.
                let pcm = unsafe { slice::from_raw_parts_mut(buffer.data, self.frames * width) };
                mix_output(pcm, source, &selection, span, width);
            }
            base += width;
        }
    }

    fn selection(&self, map: &Binding, offset: usize, samples: usize) -> Option<Selection> {
        let channels = channels(map)?;
        if !samples.is_multiple_of(map.channels) {
            return None;
        }
        let end = offset.checked_add(samples / map.channels)?;
        (end <= self.frames).then_some(Selection {
            channels,
            frames: offset..end,
        })
    }
}

fn channels(map: &Binding) -> Option<Range<usize>> {
    let start = map.offset?;
    let end = start.checked_add(map.channels)?;
    (map.channels > 0 && end <= 2048).then_some(start..end)
}

struct Selection {
    channels: Range<usize>,
    frames: Range<usize>,
}

impl Selection {
    fn span(&self, base: usize, width: usize) -> Option<(Range<usize>, Range<usize>)> {
        let start = self.channels.start.max(base);
        let end = self.channels.end.min(base + width);
        (start < end).then(|| {
            (
                start - base..end - base,
                start - self.channels.start..end - self.channels.start,
            )
        })
    }
}

fn copy_input(
    pcm: &[f32],
    destination: &mut [f32],
    selected: &Selection,
    (native, graph): (Range<usize>, Range<usize>),
    width: usize,
) {
    let source = &pcm[selected.frames.start * width..selected.frames.end * width];
    if source.is_empty() {
        return;
    }
    if native.len() == width && graph.len() == selected.channels.len() {
        destination.copy_from_slice(source);
        return;
    }
    for (native_channel, graph_channel) in native.zip(graph) {
        for (frame, output) in destination
            .chunks_exact_mut(selected.channels.len())
            .enumerate()
        {
            output[graph_channel] = source[frame * width + native_channel];
        }
    }
}

fn mix_output(
    pcm: &mut [f32],
    source: &[f32],
    selected: &Selection,
    (native, graph): (Range<usize>, Range<usize>),
    width: usize,
) {
    let destination = &mut pcm[selected.frames.start * width..selected.frames.end * width];
    if destination.is_empty() {
        return;
    }
    if native.len() == width && graph.len() == selected.channels.len() {
        for (output, input) in destination.iter_mut().zip(source) {
            *output += input;
        }
        return;
    }
    for (native_channel, graph_channel) in native.zip(graph) {
        for (frame, input) in source.chunks_exact(selected.channels.len()).enumerate() {
            destination[frame * width + native_channel] += input[graph_channel];
        }
    }
}

// SAFETY: called only inside with_native while the native callback owns complete,
// immutable metadata and live PCM. Geometry checks do not validate allocations;
// the CoreAudio IOProc contract supplies that part of the native boundary.
unsafe fn list<'a>(pointer: *const c_void, frames: u32) -> Option<&'a [AudioBuffer]> {
    if pointer.is_null() {
        return Some(&[]);
    }
    let pointer = pointer.cast::<AudioBufferList>();
    if !pointer.is_aligned() {
        return None;
    }
    // SAFETY: read only the count field; a zero-buffer list has no first entry.
    if unsafe { ptr::addr_of!((*pointer).count).read() } == 0 {
        return Some(&[]);
    }
    // SAFETY: the native allocation has the complete trailing metadata array.
    let buffers = unsafe { audio_buffer::buffers(pointer, 2048) }?;
    let mut width = 0_u32;
    for buffer in buffers {
        width = width.checked_add(buffer.channels)?;
        if buffer.channels == 0 || width > 2048 || !valid_pcm(buffer, frames) {
            return None;
        }
    }
    Some(buffers)
}

fn valid_pcm(buffer: &AudioBuffer, frames: u32) -> bool {
    if buffer.data.is_null() {
        return true;
    }
    let bytes = u64::from(frames) * u64::from(buffer.channels) * 4;
    buffer.data.is_aligned()
        && u64::from(buffer.byte_size) == bytes
        && (buffer.data as usize)
            .checked_add(buffer.byte_size as usize)
            .is_some()
}

#[cfg(test)]
mod tests;
