//! Canonical process-tap types and entry points used by the desktop adapter.
pub(super) use mixrelay_macos_process_tap::{
    TapConfig as NativeConfig, TapSession as NativeProcessTapSession, mr_process_tap_create,
    mr_process_tap_destroy, mr_process_tap_read, mr_process_tap_read_timed,
    mr_process_tap_refresh_format, mr_process_tap_start, mr_process_tap_stats, mr_process_tap_stop,
    mr_process_tap_take_peaks, mr_process_tap_wakeup,
};

// Application enumeration is supplied by the desktop Objective-C bridge.
unsafe extern "C" {
    pub(super) fn mr_process_tap_visit_applications(
        visitor: Option<
            unsafe extern "C" fn(
                *mut std::ffi::c_void,
                *const std::ffi::c_char,
                *const std::ffi::c_char,
                *const std::ffi::c_char,
                i32,
            ),
        >,
        context: *mut std::ffi::c_void,
    ) -> mixrelay_macos_process_tap::TapError;
}
