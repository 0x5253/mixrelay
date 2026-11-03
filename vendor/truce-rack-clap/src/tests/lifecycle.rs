//! Lifecycle boundary regressions.

use super::*;

struct FakePluginState {
    host: *const clap_host,
    activate_result: AtomicBool,
    latency_changed_on_activate: AtomicBool,
    request_restart_on_activate: AtomicBool,
    active: AtomicBool,
    activate_calls: AtomicUsize,
    latency_reads: AtomicUsize,
    activation_roles_valid: AtomicBool,
    teardown_stage: AtomicUsize,
    teardown_order_valid: AtomicBool,
    host_alive_during_destroy: AtomicBool,
    host_alive_during_deinit: AtomicBool,
}

impl FakePluginState {
    fn new(host: *const clap_host) -> Self {
        Self {
            host,
            activate_result: AtomicBool::new(true),
            latency_changed_on_activate: AtomicBool::new(false),
            request_restart_on_activate: AtomicBool::new(false),
            active: AtomicBool::new(false),
            activate_calls: AtomicUsize::new(0),
            latency_reads: AtomicUsize::new(0),
            activation_roles_valid: AtomicBool::new(true),
            teardown_stage: AtomicUsize::new(0),
            teardown_order_valid: AtomicBool::new(true),
            host_alive_during_destroy: AtomicBool::new(false),
            host_alive_during_deinit: AtomicBool::new(false),
        }
    }

    fn roles_are(&self, main: bool, audio: bool) -> bool {
        if self.host.is_null() {
            return false;
        }
        // SAFETY: test state retains the live boxed host context.
        let host = unsafe { &*self.host };
        let Some(get_extension) = host.get_extension else {
            return false;
        };
        // SAFETY: the requested extension pointer is stable in HostContext.
        let raw = unsafe { get_extension(self.host, CLAP_EXT_THREAD_CHECK.as_ptr()) };
        if raw.is_null() {
            return false;
        }
        // SAFETY: get_extension returned a clap_host_thread_check by its registered id.
        let check = unsafe { &*raw.cast::<clap_host_thread_check>() };
        check
            .is_main_thread
            .is_some_and(|f| unsafe { f(self.host) })
            == main
            && check
                .is_audio_thread
                .is_some_and(|f| unsafe { f(self.host) })
                == audio
    }

    fn record_stage(&self, expected: usize) {
        let actual = self.teardown_stage.fetch_add(1, Ordering::SeqCst) + 1;
        if actual != expected {
            self.teardown_order_valid.store(false, Ordering::Relaxed);
        }
    }
}

thread_local! {
    static ENTRY_DEINIT_STATE: Cell<*const FakePluginState> = const { Cell::new(ptr::null()) };
}

/// # Safety
/// The pointer must reference a live test plugin whose `plugin_data` points to live state.
unsafe fn fake_plugin_state<'a>(plugin: *const clap_plugin) -> Option<&'a FakePluginState> {
    if plugin.is_null() {
        return None;
    }
    // SAFETY: the test owns this pointer and keeps its state alive around all fake callbacks.
    unsafe { (*plugin).plugin_data.cast::<FakePluginState>().as_ref() }
}

unsafe extern "C" fn fake_activate(
    plugin: *const clap_plugin,
    _sample_rate: f64,
    _min_frames: u32,
    _max_frames: u32,
) -> bool {
    // SAFETY: test fixture's plugin_data points to its live state.
    let Some(state) = (unsafe { fake_plugin_state(plugin) }) else {
        return false;
    };
    state.activate_calls.fetch_add(1, Ordering::Relaxed);
    state
        .activation_roles_valid
        .store(state.roles_are(true, false), Ordering::Relaxed);
    // SAFETY: host pointer and callbacks are owned by HostContext for this test.
    let Some(host) = (unsafe { state.host.as_ref() }) else {
        return false;
    };
    if state.latency_changed_on_activate.load(Ordering::Relaxed)
        && let Some(get_extension) = host.get_extension
    {
        // SAFETY: host extension lookup uses a CLAP constant id and stable host storage.
        let raw = unsafe { get_extension(state.host, CLAP_EXT_LATENCY.as_ptr()) };
        if !raw.is_null() {
            // SAFETY: the returned extension matches clap.latency.
            let latency = unsafe { &*raw.cast::<clap_sys::ext::latency::clap_host_latency>() };
            if let Some(changed) = latency.changed {
                unsafe { changed(state.host) };
            }
        }
    }
    if state.request_restart_on_activate.load(Ordering::Relaxed)
        && let Some(request_restart) = host.request_restart
    {
        unsafe { request_restart(state.host) };
    }
    let activated = state.activate_result.load(Ordering::Relaxed);
    state.active.store(activated, Ordering::Relaxed);
    activated
}

unsafe extern "C" fn fake_latency_get(plugin: *const clap_plugin) -> u32 {
    // SAFETY: test fixture's plugin_data points to its live state.
    let Some(state) = (unsafe { fake_plugin_state(plugin) }) else {
        return 0;
    };
    if !state.active.load(Ordering::Relaxed) || !state.roles_are(true, false) {
        state.activation_roles_valid.store(false, Ordering::Relaxed);
    }
    u32::try_from(state.latency_reads.fetch_add(1, Ordering::Relaxed)).unwrap() + 64
}

unsafe extern "C" fn fake_stop_processing(plugin: *const clap_plugin) {
    // SAFETY: test fixture's plugin_data points to its live state.
    if let Some(state) = unsafe { fake_plugin_state(plugin) } {
        if !state.roles_are(true, true) {
            state.teardown_order_valid.store(false, Ordering::Relaxed);
        }
        state.record_stage(3);
    }
}

unsafe extern "C" fn fake_deactivate(plugin: *const clap_plugin) {
    // SAFETY: test fixture's plugin_data points to its live state.
    if let Some(state) = unsafe { fake_plugin_state(plugin) } {
        if !state.roles_are(true, false) {
            state.teardown_order_valid.store(false, Ordering::Relaxed);
        }
        state.active.store(false, Ordering::Relaxed);
        state.record_stage(4);
    }
}

unsafe extern "C" fn fake_hide(plugin: *const clap_plugin) -> bool {
    // SAFETY: test fixture's plugin_data points to its live state.
    if let Some(state) = unsafe { fake_plugin_state(plugin) } {
        if !state.roles_are(true, false) {
            state.teardown_order_valid.store(false, Ordering::Relaxed);
        }
        state.record_stage(1);
    }
    true
}

unsafe extern "C" fn fake_gui_destroy(plugin: *const clap_plugin) {
    // SAFETY: test fixture's plugin_data points to its live state.
    if let Some(state) = unsafe { fake_plugin_state(plugin) } {
        if !state.roles_are(true, false) {
            state.teardown_order_valid.store(false, Ordering::Relaxed);
        }
        state.record_stage(2);
    }
}

unsafe extern "C" fn fake_plugin_destroy(plugin: *const clap_plugin) {
    // SAFETY: test fixture's plugin_data points to its live state.
    if let Some(state) = unsafe { fake_plugin_state(plugin) } {
        state
            .host_alive_during_destroy
            .store(state.roles_are(true, false), Ordering::Relaxed);
        state.record_stage(5);
    }
}

unsafe extern "C" fn fake_entry_deinit() {
    ENTRY_DEINIT_STATE.with(|slot| {
        let state = slot.get();
        // SAFETY: test installs this pointer until after destroy_instance calls deinit.
        if let Some(state) = unsafe { state.as_ref() } {
            state
                .host_alive_during_deinit
                .store(state.roles_are(true, false), Ordering::Relaxed);
            state.record_stage(6);
        }
    });
}

fn fake_plugin(state: &mut FakePluginState) -> clap_plugin {
    // SAFETY: zero is valid for the nullable pointers and optional callbacks in this C record.
    let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
    plugin.plugin_data = (&raw mut *state).cast();
    plugin.activate = Some(fake_activate);
    plugin.stop_processing = Some(fake_stop_processing);
    plugin.deactivate = Some(fake_deactivate);
    plugin.destroy = Some(fake_plugin_destroy);
    plugin
}

fn fake_activation_config() -> ActivationConfig {
    ActivationConfig {
        layout: BusLayout::stereo(),
        sample_rate: 48_000.0,
        max_block_size: 512,
    }
}

fn fake_activation_target(
    plugin: *const clap_plugin,
    host_context: &HostContext,
    latency_ext: *const clap_plugin_latency,
) -> ActivationTarget<'_> {
    ActivationTarget {
        plugin,
        host_context,
        params_ext: ptr::null(),
        latency_ext,
    }
}

#[test]
fn activation_changed_notification_is_absorbed_but_restart_during_activation_is_preserved() {
    let host = HostContext::new();
    let mut state = FakePluginState::new(host.host_ptr());
    state
        .latency_changed_on_activate
        .store(true, Ordering::Relaxed);
    state
        .request_restart_on_activate
        .store(true, Ordering::Relaxed);
    let plugin = fake_plugin(&mut state);
    let latency = clap_plugin_latency {
        get: Some(fake_latency_get),
    };
    let start_generation = host.restart_generation();
    let mut visible_latency = Some(999);

    let snapshot = fake_activation_target(&raw const plugin, &host, &raw const latency)
        .activate_with_cache(&fake_activation_config(), &mut visible_latency)
        .unwrap();

    assert_eq!(state.activate_calls.load(Ordering::Relaxed), 1);
    assert_eq!(state.latency_reads.load(Ordering::Relaxed), 1);
    assert!(state.activation_roles_valid.load(Ordering::Relaxed));
    assert_eq!(visible_latency, Some(64));
    assert_eq!(
        snapshot.latency_generation,
        host.latency_changed_generation()
    );
    assert_eq!(host.latency_changed_generation(), 1);
    assert_eq!(host.restart_generation(), start_generation + 1);
    assert_eq!(snapshot.parameter_ids.len(), 0);
}

#[test]
fn failed_activation_keeps_latency_unknown_and_skips_native_latency_getter() {
    let host = HostContext::new();
    let mut state = FakePluginState::new(host.host_ptr());
    state.activate_result.store(false, Ordering::Relaxed);
    let plugin = fake_plugin(&mut state);
    let latency = clap_plugin_latency {
        get: Some(fake_latency_get),
    };
    let mut visible_latency = Some(77);

    let result = fake_activation_target(&raw const plugin, &host, &raw const latency)
        .activate_with_cache(&fake_activation_config(), &mut visible_latency);

    assert!(result.is_err());
    assert_eq!(visible_latency, None);
    assert_eq!(state.latency_reads.load(Ordering::Relaxed), 0);
    assert!(!state.active.load(Ordering::Relaxed));
    assert!(!(&raw const plugin).is_null());
}

#[test]
fn failed_activation_with_open_gui_still_allows_ordered_host_and_plugin_cleanup() {
    let host = HostContext::new();
    let host_ptr = host.host_ptr();
    // SAFETY: the host extension lookup uses its stable CLAP timer-support identifier.
    let timer_host =
        unsafe { ((*host_ptr).get_extension.unwrap())(host_ptr, CLAP_EXT_TIMER_SUPPORT.as_ptr()) }
            .cast::<clap_host_timer_support>();
    let mut timer_id = clap_sys::id::CLAP_INVALID_ID;
    // SAFETY: the host context remains live on its owner thread through ordered teardown.
    assert!(unsafe { ((*timer_host).register_timer.unwrap())(host_ptr, 33, &raw mut timer_id) });
    let mut state = FakePluginState::new(host.host_ptr());
    state.activate_result.store(false, Ordering::Relaxed);
    let plugin = fake_plugin(&mut state);
    let latency = clap_plugin_latency {
        get: Some(fake_latency_get),
    };
    let mut visible_latency = Some(77);
    assert!(
        fake_activation_target(&raw const plugin, &host, &raw const latency,)
            .activate_with_cache(&fake_activation_config(), &mut visible_latency,)
            .is_err()
    );

    let mut gui: clap_plugin_gui = unsafe { std::mem::zeroed() };
    gui.hide = Some(fake_hide);
    gui.destroy = Some(fake_gui_destroy);
    let entry = clap_plugin_entry {
        clap_version: clap_sys::version::CLAP_VERSION,
        init: None,
        deinit: Some(fake_entry_deinit),
        get_factory: None,
    };
    let mut gui_open = true;
    let mut started_processing = true;
    state.active.store(true, Ordering::Relaxed);
    ENTRY_DEINIT_STATE.with(|slot| slot.set(&raw const state));
    // SAFETY: fixture records, host, and fake callbacks all outlive this ordered teardown.
    unsafe {
        destroy_instance(InstanceDestruction {
            plugin: &raw const plugin,
            entry: &raw const entry,
            host_context: &host,
            gui_ext: &raw const gui,
            gui_open: &mut gui_open,
            started_processing: &mut started_processing,
            active: true,
        });
    }
    ENTRY_DEINIT_STATE.with(|slot| slot.set(ptr::null()));

    assert!(!gui_open);
    assert!(!started_processing);
    assert!(!state.active.load(Ordering::Relaxed));
    assert!(state.teardown_order_valid.load(Ordering::Relaxed));
    assert_eq!(state.teardown_stage.load(Ordering::Relaxed), 6);
    assert!(state.host_alive_during_destroy.load(Ordering::Relaxed));
    assert!(state.host_alive_during_deinit.load(Ordering::Relaxed));
    // SAFETY: the host extension and context remain live after plugin-instance destruction.
    assert!(!unsafe { ((*timer_host).unregister_timer.unwrap())(host_ptr, timer_id) });
    assert!(!(&raw const plugin).is_null());
}

unsafe extern "C" fn zero_latency(_plugin: *const clap_plugin) -> u32 {
    0
}

#[test]
fn latency_snapshot_distinguishes_absent_extension_from_incomplete_extension() {
    // SAFETY: every field in this C ABI test value is a nullable pointer, integer, or optional
    // function pointer; all-zero is a valid inert plugin record used only as a callback token.
    let plugin: clap_plugin = unsafe { std::mem::zeroed() };
    let plugin_ptr = &raw const plugin;
    // SAFETY: the absent-extension branch does not dereference the valid inert plugin token.
    assert_eq!(
        unsafe { query_latency_frames(plugin_ptr, ptr::null()) },
        Some(0)
    );

    let extension = clap_plugin_latency { get: None };
    assert_eq!(
        // SAFETY: the inert plugin token and stack extension remain live for this call.
        unsafe { query_latency_frames(plugin_ptr, &raw const extension) },
        None
    );

    let extension = clap_plugin_latency {
        get: Some(zero_latency),
    };
    assert_eq!(
        // SAFETY: the callback ignores the valid inert plugin token and returns a scalar.
        unsafe { query_latency_frames(plugin_ptr, &raw const extension) },
        Some(0)
    );
}
