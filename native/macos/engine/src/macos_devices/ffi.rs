//! `CoreAudio` C ABI. Ownership and thread rules live in native.rs.
pub(super) use mixrelay_macos_transport::device_io::{ClockBreakRecord, GraphRead, PcmPhase};
use std::ffi::{c_char, c_void};
unsafe extern "C" {
    pub(super) fn mr_device_io_next_clock_break(
        io: *const c_void,
        cursor: u64,
        record: *mut ClockBreakRecord,
    ) -> bool;
    pub(super) fn mr_device_io_pcm_hook(
        io: *mut c_void,
        context: *mut c_void,
        hook: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    );
    pub(super) fn mr_device_io_render_hook(
        io: *mut c_void,
        context: *mut c_void,
        hook: unsafe extern "C" fn(
            *mut c_void,
            *const c_void,
            *mut c_void,
            u32,
            *const mixrelay_macos_transport::device_io::IoTiming,
        ) -> bool,
    );
    pub(super) fn mr_device_io_direct_binding(
        io: *mut c_void,
        device: *mut u32,
        offset: *mut u32,
        total: *mut u32,
    ) -> bool;
    pub(super) fn mr_aggregate_group_create(
        uids: *const *const c_char,
        members: *const super::group_plan::MemberInfo,
        count: u32,
        primary: u32,
        status: *mut i32,
    ) -> *mut c_void;
    pub(super) fn mr_aggregate_member_info(
        uid: *const c_char,
        info: *mut super::group_plan::MemberInfo,
    ) -> bool;
    pub(super) fn mr_aggregate_group_try_destroy(group: *mut c_void) -> bool;
    pub(super) fn mr_aggregate_group_ready(group: *mut c_void) -> bool;
    pub(super) fn mr_device_io_create_group(
        group: *mut c_void,
        uid: *const c_char,
        input: bool,
        channels: u32,
        rate: u32,
    ) -> *mut c_void;
    pub(super) fn mr_device_io_connect_status(io: *mut c_void, expected_device: u32) -> i32;
    pub(super) fn mr_device_io_buffer_info(
        io: *mut c_void,
        info: *mut super::buffer_control::BufferInfo,
    ) -> bool;
    pub(super) fn mr_device_buffer_write(device: u32, frames: u32) -> bool;
    pub(super) fn mr_device_buffer_size(device: u32) -> u32;
    pub(super) fn mr_device_io_disconnect(io: *mut c_void) -> bool;
    pub(super) fn mr_device_io_destroy(io: *mut c_void) -> bool;
    pub(super) fn mr_device_io_alive(io: *mut c_void) -> bool;
    pub(super) fn mr_device_io_invalid(io: *mut c_void) -> bool;
    pub(super) fn mr_device_io_ticks(io: *mut c_void) -> u64;
    pub(super) fn mr_device_io_native_frames(io: *mut c_void) -> u64;
    pub(super) fn mr_device_io_native_rate(io: *mut c_void) -> u32;
    pub(super) fn mr_device_io_clock_device(io: *mut c_void) -> u32;
    pub(super) fn mr_device_io_clock_discontinuities(io: *mut c_void) -> u64;
    pub(super) fn mr_device_io_graph_frames(io: *mut c_void) -> u64;
    pub(super) fn mr_device_io_overflows(io: *mut c_void) -> u64;
    pub(super) fn mr_device_io_quantum(io: *mut c_void, graph: bool) -> u32;
    pub(super) fn mr_device_io_queued(io: *mut c_void, graph: bool) -> u32;
    pub(super) fn mr_device_io_starvations(io: *mut c_void, graph: bool) -> u64;
    pub(super) fn mr_device_io_bridge_discard(io: *mut c_void);
    pub(super) fn mr_device_io_output_epoch(io: *mut c_void, epoch: u64);
    pub(super) fn mr_device_io_bridge_read(io: *mut c_void, samples: *mut f32, frames: u32) -> u32;
    #[cfg(any(test, feature = "native-lab"))]
    pub(super) fn mr_device_io_bridge_write(
        io: *mut c_void,
        samples: *const f32,
        frames: u32,
    ) -> u32;
    pub(super) fn mr_device_io_graph_read(
        io: *mut c_void,
        samples: *mut f32,
        frames: u32,
        epoch: u64,
        host: u64,
    ) -> GraphRead;
    pub(super) fn mr_device_io_graph_write(
        io: *mut c_void,
        samples: *const f32,
        frames: u32,
        host: u64,
    ) -> u32;
    pub(super) fn mr_device_io_callback_host(io: *mut c_void) -> u64;
}

#[cfg(test)]
mod tests {
    use mixrelay_macos_transport::device_io::IoTiming;
    use std::mem::offset_of;

    #[test]
    fn the_c_header_declares_the_rust_callback_timing_layout() {
        assert_eq!(
            mixrelay_macos_test_support::device_io_timing_layout(),
            [
                size_of::<IoTiming>(),
                offset_of!(IoTiming, now),
                offset_of!(IoTiming, input),
                offset_of!(IoTiming, output),
            ]
        );
    }
}
