//! Stable host callbacks and per-instance CLAP thread-role tracking.

use clap_sys::ext::gui::{CLAP_EXT_GUI, clap_host_gui};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency};
use clap_sys::ext::thread_check::{CLAP_EXT_THREAD_CHECK, clap_host_thread_check};
use clap_sys::ext::timer_support::{
    CLAP_EXT_TIMER_SUPPORT, clap_host_timer_support, clap_plugin_timer_support,
};
use clap_sys::host::clap_host;
use clap_sys::id::{CLAP_INVALID_ID, clap_id};
use clap_sys::plugin::clap_plugin;
use clap_sys::version::CLAP_VERSION;

use std::cell::Cell;
use std::ffi::{CStr, c_void};
use std::marker::PhantomData;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, ThreadId};
use std::time::Instant;

use crate::timer_support::TimerRegistry;

const HOST_NAME: &[u8] = b"MixRelay\0";
const HOST_VENDOR: &[u8] = b"MixRelay\0";
const HOST_URL: &[u8] = b"https://mixrelay.io\0";
const HOST_VERSION: &[u8] = b"0.1.0\0";
// Native editor snapshots are capped at four million physical pixels on Cocoa. Keep accepted
// logical sizes within that cap on ordinary 2x displays and avoid unbounded OS window requests.
const MAX_GUI_DIMENSION: u32 = 4096;
const MAX_GUI_AREA: u32 = 1_000_000;

#[derive(Clone, Copy)]
struct ThreadRoles {
    audio: *const HostContext,
}

thread_local! {
    // Const-initialized TLS avoids lazy allocation on foreign threads. The owning main thread
    // writes its ID once while constructing HostContext, before any real-time call can occur.
    static THREAD_ID: Cell<Option<ThreadId>> = const { Cell::new(None) };
    // Audio-thread role is symbolic and instance-specific. Guards are scoped to a native call.
    static THREAD_ROLES: Cell<ThreadRoles> = const {
        Cell::new(ThreadRoles {
            audio: ptr::null(),
        })
    };
}

/// Per-instance CLAP host storage. It is boxed before its self-pointer is published to CLAP,
/// then stays owned by `ClapPlugin` through plugin destroy and entry deinit.
pub(crate) struct HostContext {
    host: clap_host,
    gui: clap_host_gui,
    latency: clap_host_latency,
    timer_support: clap_host_timer_support,
    thread_check: clap_host_thread_check,
    main_thread: ThreadId,
    timers: Cell<TimerRegistry>,
    restart_generation: AtomicU64,
    latency_changed_generation: AtomicU64,
    callback_requested: AtomicBool,
    process_requested: AtomicBool,
    gui_ready: AtomicBool,
    gui_size_request: AtomicU64,
    gui_closed: AtomicBool,
}

impl HostContext {
    pub(crate) fn new() -> Box<Self> {
        let main_thread = thread::current().id();
        THREAD_ID.with(|thread_id| match thread_id.get() {
            Some(existing) => debug_assert_eq!(existing, main_thread),
            None => thread_id.set(Some(main_thread)),
        });
        let mut context = Box::new(Self {
            host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: HOST_NAME.as_ptr().cast(),
                vendor: HOST_VENDOR.as_ptr().cast(),
                url: HOST_URL.as_ptr().cast(),
                version: HOST_VERSION.as_ptr().cast(),
                get_extension: Some(get_extension),
                request_restart: Some(request_restart),
                request_process: Some(request_process),
                request_callback: Some(request_callback),
            },
            latency: clap_host_latency {
                changed: Some(latency_changed),
            },
            timer_support: clap_host_timer_support {
                register_timer: Some(register_timer),
                unregister_timer: Some(unregister_timer),
            },
            gui: clap_host_gui {
                resize_hints_changed: Some(resize_hints_changed),
                request_resize: Some(gui_request_resize),
                request_show: Some(gui_request_show),
                request_hide: Some(gui_request_hide),
                closed: Some(gui_closed),
            },
            thread_check: clap_host_thread_check {
                is_main_thread: Some(is_main_thread),
                is_audio_thread: Some(is_audio_thread),
            },
            main_thread,
            timers: Cell::new(TimerRegistry::default()),
            restart_generation: AtomicU64::new(0),
            latency_changed_generation: AtomicU64::new(0),
            callback_requested: AtomicBool::new(false),
            process_requested: AtomicBool::new(false),
            gui_ready: AtomicBool::new(false),
            gui_size_request: AtomicU64::new(0),
            gui_closed: AtomicBool::new(false),
        });
        context.host.host_data = (&raw mut *context).cast::<c_void>();
        context
    }

    pub(crate) fn host_ptr(&self) -> *const clap_host {
        &raw const self.host
    }

    pub(crate) fn restart_generation(&self) -> u64 {
        self.restart_generation.load(Ordering::Acquire)
    }

    pub(crate) fn latency_changed_generation(&self) -> u64 {
        self.latency_changed_generation.load(Ordering::Acquire)
    }

    pub(crate) fn request_restart(&self) {
        self.restart_generation.fetch_add(1, Ordering::AcqRel);
    }

    fn notify_latency_changed(&self) {
        self.latency_changed_generation
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn take_callback_request(&self) -> bool {
        self.callback_requested.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn take_process_request(&self) -> bool {
        self.process_requested.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn reset_gui(&self) {
        self.gui_ready.store(false, Ordering::Release);
        self.gui_size_request.store(0, Ordering::Release);
        self.gui_closed.store(false, Ordering::Release);
    }

    pub(crate) fn gui_parent_attached(&self) {
        self.gui_ready.store(true, Ordering::Release);
    }

    pub(crate) fn requested_gui_size(&self) -> Option<(u32, u32)> {
        let packed = self.gui_size_request.load(Ordering::Acquire);
        if packed == 0 {
            return None;
        }
        let width = u32::try_from(packed >> 32).ok()?;
        let height = u32::try_from(packed & u64::from(u32::MAX)).ok()?;
        Some((width, height))
    }

    pub(crate) fn clear_gui_size_request(&self) {
        self.gui_size_request.store(0, Ordering::Release);
    }

    pub(crate) fn take_gui_closed(&self) -> bool {
        self.gui_closed.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn gui_closed_requested(&self) -> bool {
        self.gui_closed.load(Ordering::Acquire)
    }

    pub(crate) fn main_thread_scope(&self) -> truce_rack_core::error::Result<MainThreadScope> {
        if thread::current().id() != self.main_thread || !self.is_main_thread() {
            return Err(truce_rack_core::error::Error::Other(
                "CLAP main-thread operation called from a different OS thread".into(),
            ));
        }
        Ok(MainThreadScope {
            _not_send: PhantomData,
        })
    }

    fn is_main_thread(&self) -> bool {
        THREAD_ID.with(|thread_id| thread_id.get() == Some(self.main_thread))
    }

    fn register_timer(&self, period_ms: u32) -> Option<clap_id> {
        if !self.is_main_thread() {
            return None;
        }
        let mut timers = self.timers.get();
        let id = timers.register(period_ms, Instant::now())?;
        self.timers.set(timers);
        Some(id)
    }

    fn unregister_timer(&self, id: clap_id) -> bool {
        if !self.is_main_thread() {
            return false;
        }
        let mut timers = self.timers.get();
        if !timers.unregister(id) {
            return false;
        }
        self.timers.set(timers);
        true
    }

    pub(crate) fn clear_timers(&self) {
        let mut timers = self.timers.get();
        timers.clear();
        self.timers.set(timers);
    }

    /// Dispatch due timer callbacks without retaining registry borrows across plugin code.
    ///
    /// The fixed-size due snapshot bounds each service pass. Each ID is checked again after
    /// earlier callbacks so a reentrant unregister cancels pending delivery for this tick.
    pub(crate) fn service_timers(
        &self,
        plugin: *const clap_plugin,
        timer_extension: *const clap_plugin_timer_support,
        now: Instant,
    ) -> usize {
        if !self.is_main_thread() || plugin.is_null() || timer_extension.is_null() {
            return 0;
        }
        // SAFETY: the cached extension belongs to this live plugin instance and is called only
        // while the instance remains owned on its creator thread.
        let Some(on_timer) = (unsafe { (*timer_extension).on_timer }) else {
            return 0;
        };

        let mut timers = self.timers.get();
        let due = timers.collect_due(now);
        self.timers.set(timers);

        let mut delivered = 0;
        for id in due.as_slice() {
            if !self.timers.get().is_registered(*id) {
                continue;
            }
            // SAFETY: the extension came from this plugin, and dispatch runs on the captured
            // main thread while its ClapPlugin owner holds exclusive instance access.
            unsafe { on_timer(plugin, *id) };
            delivered += 1;
        }
        delivered
    }

    pub(crate) fn audio_thread_scope(&self) -> AudioThreadScope {
        let identity = ptr::from_ref::<Self>(self);
        let previous = THREAD_ROLES.with(|roles| {
            let previous = roles.get();
            roles.set(ThreadRoles { audio: identity });
            previous
        });
        AudioThreadScope {
            previous,
            _not_send: PhantomData,
        }
    }

    /// Drain at most one coalesced main-thread callback. The flag is cleared before invoking the
    /// plugin so a callback requested again from `on_main_thread` remains pending for next tick.
    pub(crate) unsafe fn service_callback(
        &self,
        plugin: *const clap_plugin,
    ) -> truce_rack_core::error::Result<bool> {
        if !self.take_callback_request() {
            return Ok(false);
        }
        let Some(callback) = (unsafe { (*plugin).on_main_thread }) else {
            return Err(truce_rack_core::error::Error::Other(
                "CLAP host callback requested but plugin has no on_main_thread".into(),
            ));
        };
        // SAFETY: caller holds the instance mutex on its captured main OS thread, and the
        // plugin pointer remains owned by the enclosing ClapPlugin.
        unsafe { callback(plugin) };
        Ok(true)
    }
}

pub(crate) struct MainThreadScope {
    _not_send: PhantomData<Rc<()>>,
}

pub(crate) struct AudioThreadScope {
    previous: ThreadRoles,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for AudioThreadScope {
    fn drop(&mut self) {
        THREAD_ROLES.with(|roles| roles.set(self.previous));
    }
}

/// Resolve the stable boxed context associated with a CLAP host pointer.
///
/// # Safety
/// `host` must be null or be the host pointer supplied by this adapter to a live plugin; for a
/// non-null pointer, its `host_data` must remain valid for the duration of this call.
unsafe fn context_from_host<'a>(host: *const clap_host) -> Option<&'a HostContext> {
    if host.is_null() {
        return None;
    }
    // SAFETY: CLAP host callbacks receive the stable host pointer supplied to create_plugin;
    // the owner keeps the pointed-to context alive through plugin destroy and entry deinit.
    let context = unsafe { (*host).host_data.cast::<HostContext>() };
    // SAFETY: host_data is initialized to the owning Box address before create_plugin is called.
    unsafe { context.as_ref() }
}

unsafe extern "C" fn get_extension(
    host: *const clap_host,
    extension_id: *const std::ffi::c_char,
) -> *const c_void {
    if extension_id.is_null() {
        return ptr::null();
    }
    // SAFETY: CLAP requires a NUL-terminated extension identifier for this callback.
    let id = unsafe { CStr::from_ptr(extension_id) };
    // SAFETY: CLAP passes the host pointer originally supplied to create_plugin.
    let Some(context) = (unsafe { context_from_host(host) }) else {
        return ptr::null();
    };
    if id == CLAP_EXT_LATENCY {
        (&raw const context.latency).cast::<c_void>()
    } else if id == CLAP_EXT_GUI {
        (&raw const context.gui).cast::<c_void>()
    } else if id == CLAP_EXT_THREAD_CHECK {
        (&raw const context.thread_check).cast::<c_void>()
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        (&raw const context.timer_support).cast::<c_void>()
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn register_timer(
    host: *const clap_host,
    period_ms: u32,
    timer_id: *mut clap_id,
) -> bool {
    if timer_id.is_null() {
        return false;
    }
    // SAFETY: CLAP supplies writable storage for the output ID when it is non-null.
    unsafe { *timer_id = CLAP_INVALID_ID };
    // SAFETY: CLAP passes the host pointer originally given to create_plugin.
    let Some(context) = (unsafe { context_from_host(host) }) else {
        return false;
    };
    let Some(id) = context.register_timer(period_ms) else {
        return false;
    };
    // SAFETY: the non-null output pointer is owned by the synchronous CLAP caller.
    unsafe { *timer_id = id };
    true
}

unsafe extern "C" fn unregister_timer(host: *const clap_host, timer_id: clap_id) -> bool {
    // SAFETY: CLAP passes the host pointer originally given to create_plugin.
    let Some(context) = (unsafe { context_from_host(host) }) else {
        return false;
    };
    context.unregister_timer(timer_id)
}

unsafe extern "C" fn resize_hints_changed(_host: *const clap_host) {
    // The editor shell polls can_resize with each snapshot; it does not cache hints.
}

unsafe extern "C" fn gui_request_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    let Some(context) = (unsafe { context_from_host(host) }) else {
        return false;
    };
    if width == 0
        || height == 0
        || width > MAX_GUI_DIMENSION
        || height > MAX_GUI_DIMENSION
        || width
            .checked_mul(height)
            .is_none_or(|area| area > MAX_GUI_AREA)
        || !context.gui_ready.load(Ordering::Acquire)
    {
        return false;
    }
    // A non-main caller receives an asynchronous acknowledgement. The main-thread editor tick
    // observes this packed size and applies it to the native parent without entering plugin code.
    context.gui_size_request.store(
        (u64::from(width) << 32) | u64::from(height),
        Ordering::Release,
    );
    true
}

unsafe extern "C" fn gui_request_show(_host: *const clap_host) -> bool {
    // This embedded host cannot surface its outer editor window from a plugin callback.
    false
}

unsafe extern "C" fn gui_request_hide(_host: *const clap_host) -> bool {
    // Hiding the plugin view alone would leave the native parent window visible.
    false
}

unsafe extern "C" fn gui_closed(host: *const clap_host, _was_destroyed: bool) {
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    if let Some(context) = unsafe { context_from_host(host) }
        && context.gui_ready.swap(false, Ordering::AcqRel)
    {
        context.gui_closed.store(true, Ordering::Release);
    }
}

unsafe extern "C" fn request_restart(host: *const clap_host) {
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    if let Some(context) = unsafe { context_from_host(host) } {
        context.request_restart();
    }
}

unsafe extern "C" fn request_process(host: *const clap_host) {
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    if let Some(context) = unsafe { context_from_host(host) } {
        context.process_requested.store(true, Ordering::Release);
    }
}

unsafe extern "C" fn request_callback(host: *const clap_host) {
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    if let Some(context) = unsafe { context_from_host(host) } {
        context.callback_requested.store(true, Ordering::Release);
    }
}

unsafe extern "C" fn latency_changed(host: *const clap_host) {
    // CLAP allows this notification during activation. The adapter queries latency after
    // activate and absorbs notifications through that query; an active notification is serviced
    // by a later main-thread lifecycle tick.
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    if let Some(context) = unsafe { context_from_host(host) } {
        context.notify_latency_changed();
    }
}

unsafe extern "C" fn is_main_thread(host: *const clap_host) -> bool {
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    let Some(context) = (unsafe { context_from_host(host) }) else {
        return false;
    };
    THREAD_ID.with(|thread_id| thread_id.get() == Some(context.main_thread))
}

unsafe extern "C" fn is_audio_thread(host: *const clap_host) -> bool {
    // SAFETY: CLAP supplies the host pointer originally given to create_plugin.
    let Some(context) = (unsafe { context_from_host(host) }) else {
        return false;
    };
    let identity = ptr::from_ref::<HostContext>(context);
    THREAD_ROLES.with(|roles| roles.get().audio == identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::AtomicUsize;

    struct CallbackState {
        host: *const clap_host,
        calls: AtomicUsize,
        request_again: AtomicBool,
    }

    unsafe extern "C" fn request_callback_again(plugin: *const clap_plugin) {
        // SAFETY: test plugin_data points to a live CallbackState for the duration of the call.
        let state = unsafe { &*(*plugin).plugin_data.cast::<CallbackState>() };
        state.calls.fetch_add(1, Ordering::Relaxed);
        if state.request_again.swap(false, Ordering::AcqRel) {
            // SAFETY: the host context lives in the parent test scope.
            unsafe { ((*state.host).request_callback.unwrap())(state.host) };
        }
    }

    #[test]
    fn stable_host_extensions_and_callbacks_are_owned_per_box() {
        let context = HostContext::new();
        let host = context.host_ptr();
        // SAFETY: host is owned by the live boxed HostContext.
        let host_ref = unsafe { &*host };
        assert_eq!(host_ref.clap_version, CLAP_VERSION);
        assert!(!host_ref.name.is_null());
        assert!(!host_ref.vendor.is_null());
        assert!(!host_ref.version.is_null());
        assert_eq!(
            host_ref.host_data,
            std::ptr::from_ref(context.as_ref()).cast_mut().cast()
        );

        let get_extension = host_ref.get_extension.unwrap();
        let latency = unsafe { get_extension(host, CLAP_EXT_LATENCY.as_ptr()) };
        let gui = unsafe { get_extension(host, CLAP_EXT_GUI.as_ptr()) };
        let thread_check = unsafe { get_extension(host, CLAP_EXT_THREAD_CHECK.as_ptr()) };
        assert_eq!(latency, (&raw const context.latency).cast());
        assert_eq!(gui, (&raw const context.gui).cast());
        assert_eq!(thread_check, (&raw const context.thread_check).cast());
        assert!(unsafe { get_extension(host, c"clap.host-params".as_ptr()) }.is_null());

        unsafe { host_ref.request_restart.unwrap()(host) };
        unsafe { host_ref.request_restart.unwrap()(host) };
        assert_eq!(context.restart_generation(), 2);
        let latency = unsafe { &*latency.cast::<clap_host_latency>() };
        unsafe { latency.changed.unwrap()(host) };
        assert_eq!(context.restart_generation(), 2);
        assert_eq!(context.latency_changed_generation(), 1);
        unsafe { host_ref.request_process.unwrap()(host) };
        assert!(context.take_process_request());
        assert!(!context.take_process_request());
    }

    #[test]
    fn main_identity_is_fixed_per_creator_thread_and_audio_role_is_instance_scoped() {
        let first = HostContext::new();
        let second = HostContext::new();
        let first_host = first.host_ptr();
        let second_host = second.host_ptr();
        let first_check = unsafe {
            (*first.host_ptr()).get_extension.unwrap()(first_host, CLAP_EXT_THREAD_CHECK.as_ptr())
        }
        .cast::<clap_host_thread_check>();
        let second_check = unsafe {
            (*second.host_ptr()).get_extension.unwrap()(second_host, CLAP_EXT_THREAD_CHECK.as_ptr())
        }
        .cast::<clap_host_thread_check>();
        let first_check = unsafe { &*first_check };
        let second_check = unsafe { &*second_check };
        {
            let _main = first.main_thread_scope().unwrap();
            assert!(unsafe { first_check.is_main_thread.unwrap()(first_host) });
            assert!(unsafe { second_check.is_main_thread.unwrap()(second_host) });
            assert!(!unsafe { first_check.is_audio_thread.unwrap()(first_host) });
            {
                let _audio = first.audio_thread_scope();
                assert!(unsafe { first_check.is_main_thread.unwrap()(first_host) });
                assert!(unsafe { first_check.is_audio_thread.unwrap()(first_host) });
            }
            assert!(!unsafe { first_check.is_audio_thread.unwrap()(first_host) });
        }
        assert!(unsafe { first_check.is_main_thread.unwrap()(first_host) });

        let host_address = first_host as usize;
        let is_main = first_check.is_main_thread.unwrap();
        let main_thread_id = first.main_thread;
        let other = std::thread::spawn(move || {
            assert_ne!(std::thread::current().id(), main_thread_id);
            // SAFETY: the parent keeps the boxed host alive until this thread joins.
            let host = host_address as *const clap_host;
            assert!(!unsafe { is_main(host) });
        });
        other.join().unwrap();
    }

    #[test]
    fn callback_requests_coalesce_and_rerequest_is_deferred_to_next_service() {
        let context = HostContext::new();
        let host = context.host_ptr();
        let mut state = CallbackState {
            host,
            calls: AtomicUsize::new(0),
            request_again: AtomicBool::new(true),
        };
        let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
        plugin.plugin_data = (&raw mut state).cast();
        plugin.on_main_thread = Some(request_callback_again);
        let plugin = &raw const plugin;
        let _main = context.main_thread_scope().unwrap();
        unsafe { ((*host).request_callback.unwrap())(host) };
        unsafe { ((*host).request_callback.unwrap())(host) };
        assert!(unsafe { context.service_callback(plugin) }.unwrap());
        assert_eq!(state.calls.load(Ordering::Relaxed), 1);
        assert!(context.callback_requested.load(Ordering::Acquire));
        assert!(unsafe { context.service_callback(plugin) }.unwrap());
        assert_eq!(state.calls.load(Ordering::Relaxed), 2);
        assert!(!context.callback_requested.load(Ordering::Acquire));
        assert!(!unsafe { context.service_callback(plugin) }.unwrap());
    }

    struct ConcurrentCallbackState {
        calls: AtomicUsize,
        entered: std::sync::Arc<std::sync::Barrier>,
        acknowledged: std::sync::Arc<std::sync::Barrier>,
    }

    unsafe extern "C" fn callback_with_concurrent_rerequest(plugin: *const clap_plugin) {
        // SAFETY: the test retains this state for the plugin callback's full lifetime.
        let state = unsafe { &*(*plugin).plugin_data.cast::<ConcurrentCallbackState>() };
        if state.calls.fetch_add(1, Ordering::AcqRel) == 0 {
            state.entered.wait();
            state.acknowledged.wait();
        }
    }

    #[test]
    fn concurrent_requests_coalesce_and_a_rerequest_during_service_survives_for_next_tick() {
        let context = HostContext::new();
        let host = context.host_ptr();
        let request = unsafe { (*host).request_callback.unwrap() };
        let host_address = host as usize;
        let worker_count = 6;
        let start = std::sync::Arc::new(std::sync::Barrier::new(worker_count + 1));
        let workers = (0..worker_count)
            .map(|_| {
                let start = std::sync::Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    // SAFETY: host remains owned by the parent until all workers join.
                    unsafe { request(host_address as *const clap_host) };
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(context.take_callback_request());
        assert!(!context.take_callback_request());

        let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
        let acknowledged = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut state = ConcurrentCallbackState {
            calls: AtomicUsize::new(0),
            entered: std::sync::Arc::clone(&entered),
            acknowledged: std::sync::Arc::clone(&acknowledged),
        };
        let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
        plugin.plugin_data = (&raw mut state).cast();
        plugin.on_main_thread = Some(callback_with_concurrent_rerequest);
        let plugin = &raw const plugin;
        let _main = context.main_thread_scope().unwrap();
        unsafe { request(host) };
        let requester = std::thread::spawn(move || {
            entered.wait();
            // SAFETY: host remains owned by the parent until this requester joins.
            unsafe { request(host_address as *const clap_host) };
            acknowledged.wait();
        });

        assert!(unsafe { context.service_callback(plugin) }.unwrap());
        requester.join().unwrap();
        assert_eq!(state.calls.load(Ordering::Relaxed), 1);
        assert!(context.callback_requested.load(Ordering::Acquire));
        assert!(unsafe { context.service_callback(plugin) }.unwrap());
        assert_eq!(state.calls.load(Ordering::Relaxed), 2);
        assert!(!context.callback_requested.load(Ordering::Acquire));
    }

    #[test]
    fn callback_request_is_retained_until_a_real_plugin_callback_exists() {
        let context = HostContext::new();
        let host = context.host_ptr();
        unsafe { ((*host).request_callback.unwrap())(host) };
        let plugin: clap_plugin = unsafe { std::mem::zeroed() };
        let _main = context.main_thread_scope().unwrap();
        assert!(unsafe { context.service_callback(&raw const plugin) }.is_err());
    }

    #[test]
    fn gui_requests_require_attached_parent_and_bound_the_observed_size() {
        let context = HostContext::new();
        let host = context.host_ptr();
        let gui = &context.gui;
        let resize = gui.request_resize.unwrap();

        assert!(!unsafe { resize(ptr::null(), 800, 600) });
        assert!(!unsafe { resize(host, 800, 600) });
        context.gui_parent_attached();
        for size in [(0, 600), (800, 0), (4097, 1), (1, 4097), (2000, 2000)] {
            assert!(!unsafe { resize(host, size.0, size.1) });
        }
        assert_eq!(context.requested_gui_size(), None);
        {
            let _audio = context.audio_thread_scope();
            assert!(unsafe { resize(host, 1000, 600) });
        }
        assert_eq!(context.requested_gui_size(), Some((1000, 600)));

        let host_address = host as usize;
        let worker = std::thread::spawn(move || {
            // SAFETY: the parent retains HostContext until this worker joins.
            unsafe { resize(host_address as *const clap_host, 900, 500) }
        });
        assert!(worker.join().unwrap());
        assert_eq!(context.requested_gui_size(), Some((900, 500)));
        context.clear_gui_size_request();
        assert_eq!(context.requested_gui_size(), None);
        context.reset_gui();
        assert!(!unsafe { resize(host, 900, 500) });
    }

    #[test]
    fn unsupported_visibility_and_closed_gui_requests_never_claim_window_changes() {
        let context = HostContext::new();
        let host = context.host_ptr();
        let gui = &context.gui;
        assert!(!unsafe { gui.request_show.unwrap()(host) });
        assert!(!unsafe { gui.request_hide.unwrap()(host) });
        unsafe { gui.resize_hints_changed.unwrap()(host) };
        unsafe { gui.closed.unwrap()(ptr::null(), true) };
        assert!(!context.gui_closed_requested());

        context.gui_parent_attached();
        unsafe { gui.closed.unwrap()(host, true) };
        assert!(context.gui_closed_requested());
        assert!(!context.gui_ready.load(Ordering::Acquire));
        assert!(!unsafe { gui.request_resize.unwrap()(host, 800, 600) });
        assert!(context.take_gui_closed());
        assert!(!context.take_gui_closed());
        context.reset_gui();
        assert!(!context.gui_closed_requested());
    }

    #[test]
    fn timer_extension_registration_is_bounded_instance_scoped_and_main_thread_only() {
        let context = HostContext::new();
        let host = context.host_ptr();
        let get_extension = unsafe { (*host).get_extension.unwrap() };
        let timer_extension = unsafe { get_extension(host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) }
            .cast::<clap_host_timer_support>();
        assert_eq!(timer_extension, (&raw const context.timer_support));
        let timer_extension = unsafe { &*timer_extension };
        let register = timer_extension.register_timer.unwrap();
        let unregister = timer_extension.unregister_timer.unwrap();

        let mut id = 123;
        assert!(!unsafe { register(host, 0, &raw mut id) });
        assert_eq!(id, CLAP_INVALID_ID);
        assert!(!unsafe { register(host, 1, ptr::null_mut()) });
        assert!(!unsafe { unregister(host, CLAP_INVALID_ID) });
        assert!(!unsafe { unregister(host, 17) });

        let ids = (0..crate::timer_support::MAX_TIMERS)
            .map(|_| {
                let mut id = CLAP_INVALID_ID;
                assert!(unsafe { register(host, 1, &raw mut id) });
                id
            })
            .collect::<Vec<_>>();
        let mut overflow_id = 0;
        assert!(!unsafe { register(host, 33, &raw mut overflow_id) });
        assert_eq!(overflow_id, CLAP_INVALID_ID);
        for id in ids {
            assert!(unsafe { unregister(host, id) });
            assert!(!unsafe { unregister(host, id) });
        }

        let first_instance = HostContext::new();
        let second_instance = HostContext::new();
        let first_host = first_instance.host_ptr();
        let second_host = second_instance.host_ptr();
        let mut first_instance_id = CLAP_INVALID_ID;
        let mut second_instance_id = CLAP_INVALID_ID;
        let first_extension = unsafe {
            (*first_host).get_extension.unwrap()(first_host, CLAP_EXT_TIMER_SUPPORT.as_ptr())
        }
        .cast::<clap_host_timer_support>();
        assert!(unsafe {
            (*first_extension).register_timer.unwrap()(first_host, 33, &raw mut first_instance_id)
        });
        let second_extension = unsafe {
            (*second_host).get_extension.unwrap()(second_host, CLAP_EXT_TIMER_SUPPORT.as_ptr())
        }
        .cast::<clap_host_timer_support>();
        assert!(unsafe {
            (*second_extension).register_timer.unwrap()(
                second_host,
                33,
                &raw mut second_instance_id,
            )
        });
        assert_eq!(first_instance_id, second_instance_id);

        let register_from_worker = register;
        let host_address = host as usize;
        let worker = std::thread::spawn(move || {
            let mut worker_id = 0;
            // SAFETY: both the callback and host remain live until this worker joins.
            unsafe {
                register_from_worker(host_address as *const clap_host, 33, &raw mut worker_id)
            }
        });
        assert!(!worker.join().unwrap());
    }

    struct TimerCallbackState {
        host: *const clap_host,
        cancel_id: clap_id,
        calls: RefCell<Vec<clap_id>>,
    }

    unsafe extern "C" fn unregister_sibling_during_timer_callback(
        plugin: *const clap_plugin,
        timer_id: clap_id,
    ) {
        // SAFETY: test plugin_data points to a live TimerCallbackState for this callback.
        let state = unsafe { &*(*plugin).plugin_data.cast::<TimerCallbackState>() };
        state.calls.borrow_mut().push(timer_id);
        let timer_extension = unsafe {
            ((*state.host).get_extension.unwrap())(state.host, CLAP_EXT_TIMER_SUPPORT.as_ptr())
        }
        .cast::<clap_host_timer_support>();
        // SAFETY: the host extension belongs to the live context retained by the test.
        unsafe {
            ((*timer_extension).unregister_timer.unwrap())(state.host, state.cancel_id);
        }
    }

    #[test]
    fn due_timer_delivery_is_bounded_and_reentrant_unregistration_skips_pending_ids() {
        let context = HostContext::new();
        let host = context.host_ptr();
        let timer_extension =
            unsafe { ((*host).get_extension.unwrap())(host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) }
                .cast::<clap_host_timer_support>();
        let register = unsafe { (*timer_extension).register_timer.unwrap() };
        let mut first_id = CLAP_INVALID_ID;
        let mut sibling_id = CLAP_INVALID_ID;
        assert!(unsafe { register(host, 33, &raw mut first_id) });
        assert!(unsafe { register(host, 33, &raw mut sibling_id) });

        let mut state = TimerCallbackState {
            host,
            cancel_id: sibling_id,
            calls: RefCell::new(Vec::new()),
        };
        let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
        plugin.plugin_data = (&raw mut state).cast();
        let plugin_timer_extension = clap_sys::ext::timer_support::clap_plugin_timer_support {
            on_timer: Some(unregister_sibling_during_timer_callback),
        };
        let now = Instant::now() + std::time::Duration::from_secs(1);

        assert_eq!(
            context.service_timers(&raw const plugin, &raw const plugin_timer_extension, now,),
            1
        );
        assert_eq!(state.calls.borrow().as_slice(), &[first_id]);
        assert!(!unsafe { ((*timer_extension).unregister_timer.unwrap())(host, sibling_id) });
        assert_eq!(
            context.service_timers(&raw const plugin, &raw const plugin_timer_extension, now),
            0
        );
        assert_eq!(
            context.service_timers(
                &raw const plugin,
                &raw const plugin_timer_extension,
                now + std::time::Duration::from_millis(33),
            ),
            1
        );
        assert_eq!(state.calls.borrow().as_slice(), &[first_id, first_id]);
    }
}
