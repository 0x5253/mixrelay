use std::{cell::RefCell, sync::atomic::Ordering::Acquire};

use super::*;

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Add(u32, u32),
    Streams,
    Remove(u32, u32),
}

#[derive(Default)]
struct Observation {
    events: Vec<Event>,
    copies: Vec<(Subscription, RcBlock<Notification>)>,
    fail_add: Option<usize>,
    fail_remove: Option<Subscription>,
    notify_add: Option<usize>,
}

#[derive(Default)]
struct Native(RefCell<Observation>);

impl ListenerBackend for Native {
    fn add(&self, subscription: Subscription, block: &Block<Notification>) -> i32 {
        let mut state = self.0.borrow_mut();
        state.events.push(Event::Add(
            subscription.object,
            subscription.address.selector,
        ));
        if state.fail_add == Some(state.copies.len()) {
            return 17;
        }
        let copy = block.copy();
        if state.notify_add == Some(state.copies.len()) {
            copy.call((1, ptr::null()));
        }
        state.copies.push((subscription, copy));
        0
    }

    fn remove(&self, subscription: Subscription, block: &Block<Notification>) -> i32 {
        let mut state = self.0.borrow_mut();
        state.events.push(Event::Remove(
            subscription.object,
            subscription.address.selector,
        ));
        if state.fail_remove == Some(subscription) {
            return 19;
        }
        let index = state
            .copies
            .iter()
            .position(|(registered, copy)| {
                *registered == subscription
                    && ptr::eq::<Block<Notification>>(&raw const **copy, ptr::from_ref(block))
            })
            .expect("same native block identity");
        state.copies.remove(index);
        0
    }

    fn streams(&self, _: &WatchTarget) -> Option<StreamIds> {
        self.0.borrow_mut().events.push(Event::Streams);
        Some(StreamIds {
            values: [234; 2048],
            count: 1,
        })
    }
}

fn target() -> WatchTarget {
    WatchTarget {
        device: 123,
        scope: u32::from_be_bytes(*b"inpt"),
        limit: 64,
        unused_scope: None,
    }
}

#[test]
fn appearing_unused_streams_invalidate_either_single_direction_registration() {
    for (used, unused) in [(*b"outp", *b"inpt"), (*b"inpt", *b"outp")] {
        let backend = Native::default();
        let invalid = Arc::new(AtomicBool::new(false));
        let mut listeners = Listeners::default();
        let target = WatchTarget {
            scope: u32::from_be_bytes(used),
            unused_scope: Some(u32::from_be_bytes(unused)),
            ..target()
        };
        listeners.install(&target, &invalid, &backend).unwrap();
        let unused_list = backend
            .0
            .borrow()
            .copies
            .iter()
            .find(|(subscription, _)| {
                subscription.object == target.device
                    && subscription.address
                        == Address::new(u32::from_be_bytes(*b"stm#"), u32::from_be_bytes(unused))
            })
            .expect("unused direction topology watch")
            .1
            .clone();
        let measured = allocation_counter::measure(|| unused_list.call((123, ptr::null())));
        assert_eq!(measured.count_total, 0);
        assert!(invalid.load(Acquire));
        assert!(listeners.remove(&backend));
    }
}

#[test]
fn watchers_precede_format_reads_and_notification_is_allocation_free() {
    let backend = Native::default();
    let invalid = Arc::new(AtomicBool::new(false));
    let mut listeners = Listeners::default();
    listeners.install(&target(), &invalid, &backend).unwrap();
    assert_eq!(
        backend.0.borrow().events,
        [
            Event::Add(1, u32::from_be_bytes(*b"srst")),
            Event::Add(123, u32::from_be_bytes(*b"nsrt")),
            Event::Add(123, u32::from_be_bytes(*b"stm#")),
            Event::Add(123, u32::from_be_bytes(*b"slay")),
            Event::Add(123, u32::from_be_bytes(*b"livn")),
            Event::Streams,
            Event::Add(234, u32::from_be_bytes(*b"sfmt")),
            Event::Add(234, u32::from_be_bytes(*b"pft ")),
            Event::Add(234, u32::from_be_bytes(*b"schn"))
        ]
    );
    let callback = backend.0.borrow().copies[4].1.clone();
    let allocation = allocation_counter::measure(|| callback.call((1, ptr::null())));
    assert_eq!(allocation.count_total, 0);
    assert!(invalid.load(Acquire));
    assert!(listeners.remove(&backend));
}

#[test]
fn prepare_notification_is_not_erased_by_later_registration() {
    let backend = Native::default();
    backend.0.borrow_mut().notify_add = Some(0);
    let invalid = Arc::new(AtomicBool::new(false));
    let mut listeners = Listeners::default();
    listeners.install(&target(), &invalid, &backend).unwrap();
    assert!(invalid.load(Acquire));
    assert!(listeners.remove(&backend));
}

#[test]
fn partial_registration_and_failed_removal_keep_exact_identity_until_retry() {
    let backend = Native::default();
    backend.0.borrow_mut().fail_add = Some(3);
    let invalid = Arc::new(AtomicBool::new(false));
    let mut listeners = Listeners::default();
    assert_eq!(listeners.install(&target(), &invalid, &backend), Err(17));
    assert_eq!(listeners.subscriptions.len(), 3);
    let retained = listeners.subscriptions[1];
    let identity = RcBlock::as_ptr(listeners.block.as_ref().unwrap());
    backend.0.borrow_mut().fail_remove = Some(retained);
    assert!(!listeners.remove(&backend));
    assert_eq!(listeners.subscriptions, [retained]);
    assert_eq!(RcBlock::as_ptr(listeners.block.as_ref().unwrap()), identity);
    backend.0.borrow_mut().fail_remove = None;
    assert!(listeners.remove(&backend));
    assert!(listeners.block.is_none());
    assert!(backend.0.borrow().copies.is_empty());
}

#[test]
fn late_native_block_copy_owns_only_the_atomic_after_device_teardown() {
    let io = super::super::DeviceIo::for_test(c"listener-fixture", false, 2, 48_000).unwrap();
    let invalid = Arc::new(AtomicBool::new(false));
    let backend = Native::default();
    let mut listeners = Listeners::default();
    listeners.install(&target(), &invalid, &backend).unwrap();
    let late = backend.0.borrow().copies[0].1.clone();
    assert!(listeners.remove(&backend));
    drop(listeners);
    drop(io);
    late.call((1, ptr::null()));
    assert!(invalid.load(Acquire));
}

#[test]
fn old_native_notification_cannot_invalidate_a_replacement_registration() {
    let backend = Native::default();
    let old = Arc::new(AtomicBool::new(false));
    let current = Arc::new(AtomicBool::new(false));
    let mut listeners = Listeners::default();
    listeners.install(&target(), &old, &backend).unwrap();
    let late = backend.0.borrow().copies[0].1.clone();
    assert!(listeners.remove(&backend));
    listeners.install(&target(), &current, &backend).unwrap();
    late.call((1, ptr::null()));
    assert!(old.load(Acquire));
    assert!(!current.load(Acquire));
    backend.0.borrow().copies[0].1.call((1, ptr::null()));
    assert!(current.load(Acquire));
    assert!(listeners.remove(&backend));
}

#[test]
fn same_width_integer_notification_blocks_cached_float_callback_without_polling() {
    use crate::{
        audio_buffer::AudioBuffer,
        device_io::{DeviceIo, mr_device_io_process},
    };

    #[repr(C)]
    struct Input {
        count: u32,
        buffer: AudioBuffer,
    }

    let io = DeviceIo::for_test(c"codec-fixture", true, 2, 48_000).unwrap();
    let invalid = Arc::new(AtomicBool::new(false));
    // SAFETY: synchronous unpublished fixture; no native IOProc can access its cell.
    unsafe {
        let callback = &mut *io.callback.get();
        assert!(
            callback
                .formats
                .append(2, false, mixrelay_pcm_ring::SampleFormat::Float32)
        );
        callback.watch_invalid = invalid.clone();
    }
    let backend = Native::default();
    let mut listeners = Listeners::default();
    listeners.install(&target(), &invalid, &backend).unwrap();
    let notification = backend.0.borrow().copies[5].1.clone();
    // Actual Signed32 bytes have the same channels/stride as the cached Float32 ASBD.
    let mut samples = [i32::MAX; 128];
    let input = Input {
        count: 1,
        buffer: AudioBuffer {
            channels: 2,
            byte_size: 512,
            data: samples.as_mut_ptr().cast(),
        },
    };
    let allocation = allocation_counter::measure(|| {
        notification.call((1, ptr::null()));
        // SAFETY: complete native metadata/PCM retained for this serialized callback.
        unsafe {
            mr_device_io_process(
                123,
                ptr::null(),
                ptr::from_ref(&input).cast(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                ptr::from_ref(&io).cast_mut().cast(),
            );
        }
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(io.native_ring.stats().queued, 0);
    assert_eq!(io.progress.ticks.load(Acquire), 0);
    assert_eq!(samples, [i32::MAX; 128]);
    assert!(listeners.remove(&backend));
}

#[test]
fn failed_native_listener_removal_keeps_device_allocation_until_acknowledged() {
    use crate::device_io::{DeviceIo, ffi::destroy_with};

    let io = DeviceIo::for_test(c"retained-listener", false, 2, 48_000)
        .unwrap()
        .publish();
    let backend = Native::default();
    let invalid = Arc::new(AtomicBool::new(false));
    // SAFETY: fixture is the sole allocation/control owner, with no registered IOProc.
    let control = unsafe { &mut *(*io).control.get() };
    control
        .listeners
        .install(&target(), &invalid, &backend)
        .unwrap();
    backend.0.borrow_mut().fail_remove = Some(control.listeners.subscriptions[0]);
    let remove = |owner: &DeviceIo| {
        // SAFETY: destroy_with serializes this control cell after all queue roles have ended.
        unsafe { &mut *owner.control.get() }
            .listeners
            .remove(&backend)
    };
    // SAFETY: only this fixture owns the stable pointer and no callback is registered.
    assert!(!unsafe { destroy_with(io, remove) });
    // SAFETY: failed acknowledged removal retained the original allocation.
    assert_eq!(unsafe { &*io }.selection.channels, 2);
    backend.0.borrow_mut().fail_remove = None;
    // SAFETY: retry acknowledges the same native identities before freeing the allocation.
    assert!(unsafe { destroy_with(io, remove) });
    assert!(backend.0.borrow().copies.is_empty());
}

#[test]
fn device_removal_silences_duplex_callback_before_control_polling() {
    use crate::{
        audio_buffer::AudioBuffer,
        device_io::{DeviceIo, mr_device_io_process},
    };

    #[repr(C)]
    struct Buffers {
        count: u32,
        buffer: AudioBuffer,
    }

    for input in [true, false] {
        let io = DeviceIo::for_test(c"removed-device", input, 2, 48_000).unwrap();
        let invalid = Arc::new(AtomicBool::new(false));
        // SAFETY: unpublished fixture has no native registration or concurrent callback.
        unsafe {
            let callback = &mut *io.callback.get();
            assert!(
                callback
                    .formats
                    .append(2, false, mixrelay_pcm_ring::SampleFormat::Float32)
            );
            callback.watch_invalid = invalid.clone();
        }
        let backend = Native::default();
        let mut listeners = Listeners::default();
        listeners.install(&target(), &invalid, &backend).unwrap();
        let notification = backend
            .0
            .borrow()
            .copies
            .iter()
            .find(|(subscription, _)| {
                subscription.object == 123
                    && subscription.address == Address::new(u32::from_be_bytes(*b"livn"), GLOBAL)
            })
            .expect("device removal watch")
            .1
            .clone();
        let mut source = [0.5_f32; 128];
        let mut destination = [0.75_f32; 128];
        let source_buffers = Buffers {
            count: 1,
            buffer: AudioBuffer {
                channels: 2,
                byte_size: 512,
                data: source.as_mut_ptr().cast(),
            },
        };
        let mut destination_buffers = Buffers {
            count: 1,
            buffer: AudioBuffer {
                channels: 2,
                byte_size: 512,
                data: destination.as_mut_ptr().cast(),
            },
        };
        let allocation = allocation_counter::measure(|| {
            notification.call((1, ptr::null()));
            // SAFETY: complete duplex metadata and PCM remain live for this serialized call.
            unsafe {
                mr_device_io_process(
                    123,
                    ptr::null(),
                    ptr::from_ref(&source_buffers).cast(),
                    ptr::null(),
                    ptr::from_mut(&mut destination_buffers).cast(),
                    ptr::null(),
                    ptr::from_ref(&io).cast_mut().cast(),
                );
            }
        });
        assert_eq!(allocation.count_total, 0);
        assert_eq!(destination.map(f32::to_bits), [0; 128]);
        assert_eq!(source.map(f32::to_bits), [0.5_f32.to_bits(); 128]);
        assert_eq!(io.native_ring.stats().queued, 0);
        assert_eq!(io.progress.ticks.load(Acquire), 0);
        assert!(listeners.remove(&backend));
        assert!(backend.0.borrow().copies.is_empty());
    }
}

#[test]
fn device_removal_during_render_hook_discards_its_written_output() {
    use crate::{
        audio_buffer::AudioBuffer,
        device_io::{DeviceIo, mr_device_io_process},
    };

    unsafe extern "C" fn render_then_remove(
        context: *mut c_void,
        _: *const c_void,
        output: *mut c_void,
        _: u32,
        _: *const crate::device_io::IoTiming,
    ) -> bool {
        // SAFETY: the fixture keeps its complete writable native output list and block live.
        let buffers = unsafe { crate::audio_buffer::buffers(output.cast(), 1) }.unwrap();
        let samples = unsafe { std::slice::from_raw_parts_mut(buffers[0].data.cast::<f32>(), 128) };
        samples.fill(0.75);
        unsafe { &*context.cast::<RcBlock<Notification>>() }.call((1, ptr::null()));
        true
    }

    #[repr(C)]
    struct Buffers {
        count: u32,
        buffer: AudioBuffer,
    }
    let io = DeviceIo::for_test(c"removed-in-hook", false, 2, 48_000).unwrap();
    let invalid = Arc::new(AtomicBool::new(false));
    let backend = Native::default();
    let mut listeners = Listeners::default();
    listeners.install(&target(), &invalid, &backend).unwrap();
    let notification = backend.0.borrow().copies[4].1.clone();
    // SAFETY: fixture is unpublished and keeps the hook block alive until the call ends.
    unsafe {
        let callback = &mut *io.callback.get();
        assert!(
            callback
                .formats
                .append(2, false, mixrelay_pcm_ring::SampleFormat::Float32)
        );
        callback.watch_invalid = invalid.clone();
        callback.hook = Some(render_then_remove);
        callback.context = ptr::from_ref(&notification).cast_mut().cast();
    }
    let mut samples = [0.5_f32; 128];
    let mut output = Buffers {
        count: 1,
        buffer: AudioBuffer {
            channels: 2,
            byte_size: 512,
            data: samples.as_mut_ptr().cast(),
        },
    };
    let allocation = allocation_counter::measure(|| {
        // SAFETY: serialized call owns the complete native metadata, PCM and hook context.
        unsafe {
            mr_device_io_process(
                123,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::from_mut(&mut output).cast(),
                ptr::null(),
                ptr::from_ref(&io).cast_mut().cast(),
            );
        }
    });
    assert!(invalid.load(Acquire));
    assert_eq!(allocation.count_total, 0);
    assert_eq!(samples.map(f32::to_bits), [0; 128]);
    assert!(io.progress.invalid.load(Acquire));
    assert_eq!(io.progress.ticks.load(Acquire), 0);
    assert!(listeners.remove(&backend));
}
