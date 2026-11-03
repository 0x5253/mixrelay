//! Parameters boundary regressions.

use super::*;

struct ParameterFixture {
    get_info_calls: AtomicUsize,
    flush_calls: AtomicUsize,
    flush_event_count: AtomicUsize,
    count: u32,
    fail_index: Option<u32>,
    requires_process_index: Option<u32>,
}

unsafe extern "C" fn fake_parameter_count(plugin: *const clap_plugin) -> u32 {
    // SAFETY: test fixture's plugin_data points to the live parameter state.
    unsafe { &*(*plugin).plugin_data.cast::<ParameterFixture>() }.count
}

unsafe extern "C" fn fake_parameter_info(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    // SAFETY: test fixture's plugin_data and output record remain live for the call.
    let state = unsafe { &*(*plugin).plugin_data.cast::<ParameterFixture>() };
    state.get_info_calls.fetch_add(1, Ordering::Relaxed);
    if state.fail_index == Some(index) || index >= state.count || info.is_null() {
        return false;
    }
    unsafe {
        (*info).id = 1000 + index;
        if state.requires_process_index == Some(index) {
            (*info).flags |= CLAP_PARAM_REQUIRES_PROCESS;
        }
    }
    true
}

unsafe extern "C" fn fake_parameter_flush(
    plugin: *const clap_plugin,
    inputs: *const clap_input_events,
    _outputs: *const clap_output_events,
) {
    // SAFETY: the plugin fixture and input event list remain live for this synchronous call.
    let state = unsafe { &*(*plugin).plugin_data.cast::<ParameterFixture>() };
    let input_events = unsafe { &*inputs };
    let count = input_events.size.map_or(0, |size| unsafe { size(inputs) });
    state.flush_calls.fetch_add(1, Ordering::Relaxed);
    state
        .flush_event_count
        .store(count as usize, Ordering::Relaxed);
}

#[test]
fn parameter_id_cache_uses_native_get_info_only_during_main_thread_refresh() {
    let host = HostContext::new();
    let mut fixture = ParameterFixture {
        get_info_calls: AtomicUsize::new(0),
        flush_calls: AtomicUsize::new(0),
        flush_event_count: AtomicUsize::new(0),
        count: 2,
        fail_index: None,
        requires_process_index: Some(1),
    };
    let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
    plugin.plugin_data = (&raw mut fixture).cast();
    let params = clap_plugin_params {
        count: Some(fake_parameter_count),
        get_info: Some(fake_parameter_info),
        get_value: None,
        value_to_text: None,
        text_to_value: None,
        flush: Some(fake_parameter_flush),
    };
    let _main_thread = host.main_thread_scope().unwrap();
    let ids = read_parameter_ids(&raw const plugin, &raw const params).unwrap();
    assert_eq!(ids.id(0).unwrap(), 1000);
    assert_eq!(ids.id(1).unwrap(), 1001);
    assert_eq!(fixture.get_info_calls.load(Ordering::Relaxed), 2);

    let mut ids = ids;
    queue_cached_parameter_change(&mut ids, 0, 0.25).unwrap();
    queue_cached_parameter_change(&mut ids, 1, 0.625).unwrap();
    assert!(ids.has_pending());
    assert!(ids.has_flushable_pending());

    assert!(
        flush_pending_parameter_changes(&raw const plugin, &raw const params, &host, &mut ids,)
            .unwrap()
    );
    assert_eq!(fixture.flush_calls.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.flush_event_count.load(Ordering::Relaxed), 1);

    let mut pending = Vec::new();
    ids.for_each_pending(|id, value| pending.push((id, value)));
    assert_eq!(pending, [(1001, 0.625)]);
    assert_eq!(ids.pending_count(), 1);
    assert_eq!(fixture.get_info_calls.load(Ordering::Relaxed), 2);

    let mut process_delivery = Vec::new();
    ids.for_each_pending(|id, value| process_delivery.push((id, value)));
    assert_eq!(process_delivery, [(1001, 0.625)]);
    commit_processed_parameter_changes(&mut ids, CLAP_PROCESS_ERROR);
    assert!(ids.has_pending());
    commit_processed_parameter_changes(&mut ids, CLAP_PROCESS_CONTINUE);
    assert!(!ids.has_pending());
}

#[test]
fn parameter_id_refresh_failure_is_explicit() {
    let host = HostContext::new();
    let mut fixture = ParameterFixture {
        get_info_calls: AtomicUsize::new(0),
        flush_calls: AtomicUsize::new(0),
        flush_event_count: AtomicUsize::new(0),
        count: 2,
        fail_index: Some(1),
        requires_process_index: None,
    };
    let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
    plugin.plugin_data = (&raw mut fixture).cast();
    let params = clap_plugin_params {
        count: Some(fake_parameter_count),
        get_info: Some(fake_parameter_info),
        get_value: None,
        value_to_text: None,
        text_to_value: None,
        flush: None,
    };
    let _main_thread = host.main_thread_scope().unwrap();
    assert!(matches!(
        read_parameter_ids(&raw const plugin, &raw const params),
        Err(Error::Other(message)) if message.contains("index 1")
    ));
}
