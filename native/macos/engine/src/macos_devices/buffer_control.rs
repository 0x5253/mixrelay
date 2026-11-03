//! Rust-owned client buffer validation and acknowledgement. Native functions
//! expose snapshots and one setter call; no retry/format policy lives in C.
use super::{buffer_transaction::BufferControl, host::Host, native::Native};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

#[repr(C)]
#[derive(Default)]
pub(super) struct BufferInfo {
    device: u32,
    frames: u32,
    minimum: f64,
    maximum: f64,
    settable: u32,
}
impl BufferInfo {
    fn accepts(&self, requested: u32) -> bool {
        self.device != 0
            && self.frames != 0
            && requested != 0
            && (self.frames == requested
                || (self.settable == 1
                    && self.minimum.is_finite()
                    && self.maximum.is_finite()
                    && self.minimum <= f64::from(requested)
                    && f64::from(requested) <= self.maximum))
    }
}

pub(super) fn prepare(native: &Native, requested: u32) -> Result<(u32, u32), String> {
    let mut info = BufferInfo::default();
    let available = native.host.buffer_info(native, &mut info);
    validate_snapshot(&info, available, requested)
}

pub(super) fn prepare_initial(
    native: &Native,
    requested: u32,
    stop: &AtomicBool,
) -> Result<(u32, u32), String> {
    await_selection(
        requested,
        || {
            let mut info = BufferInfo::default();
            let available = native.host.buffer_info(native, &mut info);
            (info, available)
        },
        || stop.load(Ordering::Acquire),
        || thread::park_timeout(Duration::from_millis(10)),
    )
}

// A creator process's acknowledgement does not imply that this process can
// already resolve the UID. Bound only that discovery phase (at most 49 waits).
// Once selected, a bad property/range fails immediately. No setter, IOProc or
// rejected graph preparation is retried here; format edits use prepare directly.
fn await_selection(
    requested: u32,
    mut snapshot: impl FnMut() -> (BufferInfo, bool),
    mut cancelled: impl FnMut() -> bool,
    mut wait: impl FnMut(),
) -> Result<(u32, u32), String> {
    for attempt in 0..50 {
        if cancelled() {
            return Err("CoreAudio device discovery cancelled".into());
        }
        let (info, available) = snapshot();
        if available || info.device != 0 || attempt == 49 {
            return validate_snapshot(&info, available, requested);
        }
        wait();
    }
    unreachable!("final discovery snapshot always returns")
}

fn validate_snapshot(
    info: &BufferInfo,
    available: bool,
    requested: u32,
) -> Result<(u32, u32), String> {
    if !available {
        return Err(format!(
            "CoreAudio client buffer snapshot unavailable: device={} frames={}",
            info.device, info.frames
        ));
    }
    if !info.accepts(requested) {
        return Err(format!(
            "CoreAudio client buffer {requested} rejected: device={} current={} range={}..{} settable={}",
            info.device, info.frames, info.minimum, info.maximum, info.settable
        ));
    }
    Ok((info.device, info.frames))
}

/// Client buffer settings of the devices behind one host.
pub(super) struct HostBufferControl<'a>(pub(super) &'a dyn Host);
impl BufferControl for HostBufferControl<'_> {
    fn current(&mut self, device: u32) -> u32 {
        self.0.buffer_size(device)
    }
    fn change(&mut self, device: u32, expected: u32, requested: u32) -> bool {
        confirmed_write(
            expected,
            requested,
            || self.0.buffer_size(device),
            || self.0.write_buffer(device, requested),
            || thread::sleep(Duration::from_millis(2)),
        )
    }
}

fn confirmed_write(
    expected: u32,
    requested: u32,
    mut read: impl FnMut() -> u32,
    mut write: impl FnMut() -> bool,
    mut wait: impl FnMut(),
) -> bool {
    if requested == 0 || read() != expected {
        return false;
    }
    if expected == requested {
        return true;
    }
    if !write() {
        return false;
    }
    for attempt in 0..50 {
        if read() == requested {
            return true;
        }
        if attempt < 49 {
            wait();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_waits_only_for_unpublished_selection_and_is_bounded() {
        let mut reads = 0;
        let mut waits = 0;
        assert_eq!(
            await_selection(
                32,
                || {
                    reads += 1;
                    if reads < 3 {
                        (BufferInfo::default(), false)
                    } else {
                        (
                            BufferInfo {
                                device: 7,
                                frames: 32,
                                ..BufferInfo::default()
                            },
                            true,
                        )
                    }
                },
                || false,
                || waits += 1
            )
            .unwrap(),
            (7, 32)
        );
        assert_eq!((reads, waits), (3, 2));
        reads = 0;
        waits = 0;
        assert!(
            await_selection(
                32,
                || {
                    reads += 1;
                    (BufferInfo::default(), false)
                },
                || false,
                || waits += 1
            )
            .is_err()
        );
        assert_eq!((reads, waits), (50, 49));
    }
    #[test]
    fn discovery_never_retries_known_device_failure_or_ignores_cancellation() {
        for available in [false, true] {
            assert!(
                await_selection(
                    32,
                    || (
                        BufferInfo {
                            device: 7,
                            frames: 256,
                            ..BufferInfo::default()
                        },
                        available
                    ),
                    || false,
                    || panic!("known device failure must not wait")
                )
                .is_err()
            );
        }
        assert!(
            await_selection(
                32,
                || panic!("cancelled discovery must not query"),
                || true,
                || panic!("cancelled discovery must not wait")
            )
            .is_err()
        );
    }
    #[test]
    fn requested_size_requires_native_support_but_unchanged_size_needs_no_setter() {
        assert_eq!(std::mem::size_of::<BufferInfo>(), 32);
        assert_eq!(std::mem::offset_of!(BufferInfo, settable), 24);
        let mut info = BufferInfo {
            device: 1,
            frames: 256,
            minimum: 32.0,
            maximum: 512.0,
            settable: 1,
        };
        assert!(info.accepts(32));
        assert!(info.accepts(512));
        assert!(!info.accepts(0));
        assert!(!info.accepts(16));
        assert!(!info.accepts(1024));
        info.settable = 0;
        assert!(!info.accepts(32));
        assert!(info.accepts(256));
        info.settable = 1;
        info.maximum = f64::NAN;
        assert!(!info.accepts(32));
    }
    #[test]
    fn stale_snapshot_and_no_op_never_write() {
        assert!(!confirmed_write(
            256,
            32,
            || 128,
            || panic!("external setting overwritten"),
            || {}
        ));
        assert!(confirmed_write(
            256,
            256,
            || 256,
            || panic!("no-op write"),
            || {}
        ));
    }
    #[test]
    fn malformed_native_geometry_rejects_changed_buffers() {
        let valid = || BufferInfo {
            device: 7,
            frames: 256,
            minimum: 32.0,
            maximum: 512.0,
            settable: 1,
        };
        for info in [
            BufferInfo {
                device: 0,
                ..valid()
            },
            BufferInfo {
                frames: 0,
                ..valid()
            },
            BufferInfo {
                minimum: f64::NAN,
                ..valid()
            },
            BufferInfo {
                maximum: f64::INFINITY,
                ..valid()
            },
            BufferInfo {
                minimum: 512.0,
                maximum: 32.0,
                ..valid()
            },
            BufferInfo {
                settable: 2,
                ..valid()
            },
        ] {
            assert!(!info.accepts(128));
            assert!(validate_snapshot(&info, true, 128).is_err());
        }
        assert!(validate_snapshot(&valid(), false, 128).is_err());
        assert_eq!(validate_snapshot(&valid(), true, 128).unwrap(), (7, 256));
    }
    #[test]
    fn acknowledgement_is_bounded_and_setter_success_alone_is_not_acceptance() {
        let mut reads = 0;
        let mut writes = 0;
        let mut waits = 0;
        assert!(!confirmed_write(
            256,
            32,
            || {
                reads += 1;
                256
            },
            || {
                writes += 1;
                true
            },
            || waits += 1
        ));
        assert_eq!((reads, writes, waits), (51, 1, 49));
        let mut reads = 0;
        assert!(confirmed_write(
            256,
            32,
            || {
                reads += 1;
                if reads < 3 { 256 } else { 32 }
            },
            || true,
            || {}
        ));
        assert_eq!(reads, 3);
    }
}
