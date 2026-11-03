//! CLAP state stream ownership and byte transfer.

use crate::ClapPlugin;
use crate::events::ConvertedInputEvents;
use crate::lifecycle::query_latency_frames;
use crate::parameters::read_parameter_ids;
use clap_sys::stream::{clap_istream, clap_ostream};
use truce_rack_core::error::{Error, Result};

/// Sink for `clap_plugin_state::save` — the plugin pushes bytes
/// via the C `write` callback, we accumulate into a Vec.
#[derive(Default)]
pub(super) struct WriteBuffer {
    pub(super) bytes: Vec<u8>,
}

pub(super) unsafe extern "C" fn ostream_write(
    stream: *const clap_ostream,
    buffer: *const std::ffi::c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let ctx = unsafe { (*stream).ctx.cast::<WriteBuffer>() };
    if ctx.is_null() {
        return -1;
    }
    let Ok(size_usize) = usize::try_from(size) else {
        return -1;
    };
    if size_usize > isize::MAX as usize {
        return -1;
    }
    // SAFETY: the live stream context owns its output storage for the save callback.
    let output = unsafe { &mut (*ctx).bytes };
    if output.try_reserve(size_usize).is_err() {
        return -1;
    }
    // SAFETY: CLAP supplies size bytes; the validated length fits Rust's slice bound.
    let slice = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), size_usize) };
    output.extend_from_slice(slice);
    i64::try_from(size_usize).unwrap_or(i64::MAX)
}

/// Source for `clap_plugin_state::load` — the plugin pulls bytes
/// via the C `read` callback, we hand back from a `&[u8]`.
pub(super) struct ReadCursor<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) position: usize,
}

pub(super) unsafe extern "C" fn istream_read(
    stream: *const clap_istream,
    buffer: *mut std::ffi::c_void,
    size: u64,
) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    let ctx = unsafe { (*stream).ctx.cast::<ReadCursor<'_>>() };
    if ctx.is_null() {
        return -1;
    }
    let cursor = unsafe { &mut *ctx };
    let Ok(want) = usize::try_from(size) else {
        return -1;
    };
    let available = cursor.bytes.len().saturating_sub(cursor.position);
    let take = want.min(available);
    if take > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(
                cursor.bytes.as_ptr().add(cursor.position),
                buffer.cast::<u8>(),
                take,
            );
        }
        cursor.position += take;
    }
    i64::try_from(take).unwrap_or(i64::MAX)
}

impl ClapPlugin {
    pub(super) fn save_native_state(&self) -> Result<Vec<u8>> {
        let _main_thread = self.host_context.main_thread_scope()?;
        if self.state_ext.is_null() {
            return Err(Error::Other("plugin missing clap.state extension".into()));
        }
        let save = unsafe { (*self.state_ext).save }
            .ok_or_else(|| Error::Other("clap.state extension missing save fn".into()))?;
        let mut buffer = WriteBuffer::default();
        let stream = clap_ostream {
            ctx: (&raw mut buffer).cast(),
            write: Some(ostream_write),
        };
        let ok = unsafe { save(self.plugin, &raw const stream) };
        if !ok {
            return Err(Error::Other("clap state save returned false".into()));
        }
        Ok(buffer.bytes)
    }

    pub(super) fn load_native_state(&mut self, bytes: &[u8]) -> Result<()> {
        let _main_thread = self.host_context.main_thread_scope()?;
        if self.state_ext.is_null() {
            return Err(Error::Other("plugin missing clap.state extension".into()));
        }
        let load = unsafe { (*self.state_ext).load }
            .ok_or_else(|| Error::Other("clap.state extension missing load fn".into()))?;
        let mut cursor = ReadCursor { bytes, position: 0 };
        let stream = clap_istream {
            ctx: (&raw mut cursor).cast(),
            read: Some(istream_read),
        };
        self.processing.latency_frames = None;
        let ok = unsafe { load(self.plugin, &raw const stream) };
        if !ok {
            return Err(Error::Other("clap state load returned false".into()));
        }
        self.parameters.ids.clear_all_pending();
        let parameter_ids = read_parameter_ids(self.plugin, self.parameters.extension)?;
        let input_events = if self.processing.active_layout.is_some() {
            Some(ConvertedInputEvents::for_process(parameter_ids.len())?)
        } else {
            None
        };
        self.parameters.ids = parameter_ids;
        self.processing.input_events = input_events;
        self.parameters.changed_pending = true;
        self.processing.latency_frames = if self.processing.active_layout.is_some() {
            // SAFETY: state load ran on the fixed main thread while the instance remains active.
            unsafe { query_latency_frames(self.plugin, self.latency_ext) }
        } else {
            None
        };
        self.processing.completed_latency_generation =
            self.host_context.latency_changed_generation();
        Ok(())
    }
}
