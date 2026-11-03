//! Editor boundary regressions.

use super::*;

#[test]
fn editor_parent_requires_the_platform_api() {
    use truce_rack_core::editor::WindowHandle;

    let pointer = std::ptr::NonNull::<u8>::dangling().as_ptr().cast();
    for (handle, expected_api) in [
        (WindowHandle::NSView(pointer), CLAP_WINDOW_API_COCOA),
        (WindowHandle::HWND(pointer), CLAP_WINDOW_API_WIN32),
        (WindowHandle::X11(u64::from(u32::MAX)), CLAP_WINDOW_API_X11),
    ] {
        let result = handle_to_clap_window(handle);
        if expected_api != platform_api() {
            assert!(result.is_err());
            continue;
        }
        let window = result.expect("matching native parent API");
        // SAFETY: conversion initializes exactly the union member selected
        // by this handle; pointer values are compared, never dereferenced.
        unsafe {
            match handle {
                WindowHandle::NSView(_) => assert_eq!(window.specific.cocoa, pointer),
                WindowHandle::HWND(_) => assert_eq!(window.specific.win32, pointer),
                WindowHandle::X11(id) => {
                    assert_eq!(u128::from(window.specific.x11), u128::from(id));
                }
            }
        }
    }
}

#[cfg(any(target_os = "windows", target_pointer_width = "32"))]
#[test]
fn oversized_x11_parent_is_rejected_without_truncation() {
    let result = handle_to_clap_window(truce_rack_core::editor::WindowHandle::X11(
        u64::from(u32::MAX) + 1,
    ));

    assert!(matches!(result, Err(Error::Other(message)) if message.contains("ABI width")));
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
fn x11_parent_preserves_the_native_unsigned_long_width() {
    let window = handle_to_clap_window(truce_rack_core::editor::WindowHandle::X11(u64::MAX))
        .expect("64-bit native X11 handle");

    // SAFETY: the X11 input initializes the x11 union member.
    assert_eq!(unsafe { window.specific.x11 }, u64::MAX);
}

#[test]
fn main_thread_service_delivers_timers_without_an_open_editor() {
    struct TimerCalls {
        count: Cell<usize>,
        last_id: Cell<clap_sys::id::clap_id>,
    }
    unsafe extern "C" fn on_timer(plugin: *const clap_plugin, timer_id: clap_sys::id::clap_id) {
        // SAFETY: plugin_data points to TimerCalls retained by this test through servicing.
        let calls = unsafe { &*(*plugin).plugin_data.cast::<TimerCalls>() };
        calls.count.set(calls.count.get() + 1);
        calls.last_id.set(timer_id);
    }

    let calls = TimerCalls {
        count: Cell::new(0),
        last_id: Cell::new(clap_sys::id::CLAP_INVALID_ID),
    };
    // SAFETY: all-zero initializes the nullable pointers and optional callback fields.
    let mut native: clap_plugin = unsafe { std::mem::zeroed() };
    native.plugin_data = std::ptr::from_ref(&calls).cast_mut().cast();
    let mut instance = active_process_test_instance(&native, process_layout(2, 2));
    assert!(!instance.editor.open);

    let host = instance.host_context.host_ptr();
    // SAFETY: host extension lookup uses the stable CLAP timer-support identifier.
    let timer_host =
        unsafe { ((*host).get_extension.unwrap())(host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) }
            .cast::<clap_host_timer_support>();
    let mut timer_id = clap_sys::id::CLAP_INVALID_ID;
    // SAFETY: the host remains live on its fixed main thread for this registration.
    assert!(unsafe { ((*timer_host).register_timer.unwrap())(host, 33, &raw mut timer_id) });
    let timer_extension = clap_plugin_timer_support {
        on_timer: Some(on_timer),
    };
    instance.timer_ext = &raw const timer_extension;

    // Timer support has a 33 ms minimum; allow one period to elapse before the owner tick.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(
        instance.service_main_thread().unwrap(),
        MainThreadUpdate::Unchanged
    );
    assert_eq!(calls.count.get(), 1);
    assert_eq!(calls.last_id.get(), timer_id);
}

#[test]
fn editor_scale_respects_logical_pixels_and_closed_gui_ignores_visibility_calls() {
    use truce_rack_core::{PluginEditor, WindowHandle};

    struct Calls {
        scale: Cell<usize>,
        visibility: Cell<usize>,
    }
    unsafe extern "C" fn create(_: *const clap_plugin, _: *const c_char, _: bool) -> bool {
        true
    }
    unsafe extern "C" fn destroy(_: *const clap_plugin) {}
    unsafe extern "C" fn parent(_: *const clap_plugin, _: *const clap_window) -> bool {
        true
    }
    unsafe extern "C" fn scale(plugin: *const clap_plugin, _: f64) -> bool {
        // SAFETY: this test's live plugin_data points to Calls; all calls are serialized.
        let calls = unsafe { &*(*plugin).plugin_data.cast::<Calls>() };
        calls.scale.set(calls.scale.get() + 1);
        true
    }
    unsafe extern "C" fn visibility(plugin: *const clap_plugin) -> bool {
        // SAFETY: this test's live plugin_data points to Calls; all calls are serialized.
        let calls = unsafe { &*(*plugin).plugin_data.cast::<Calls>() };
        calls.visibility.set(calls.visibility.get() + 1);
        true
    }
    let calls = Calls {
        scale: Cell::new(0),
        visibility: Cell::new(0),
    };
    // SAFETY: CLAP callback fields are optional and all other fields accept null pointers.
    let mut native: clap_plugin = unsafe { std::mem::zeroed() };
    native.plugin_data = std::ptr::from_ref(&calls).cast_mut().cast();
    // SAFETY: GUI fields are optional function pointers; absent callbacks are valid.
    let mut gui: clap_plugin_gui = unsafe { std::mem::zeroed() };
    gui.create = Some(create);
    gui.destroy = Some(destroy);
    gui.set_parent = Some(parent);
    gui.set_scale = Some(scale);
    gui.show = Some(visibility);
    gui.hide = Some(visibility);
    let mut instance = active_process_test_instance(&native, process_layout(2, 2));
    instance.editor.extension = &raw const gui;
    instance.show();
    instance.hide();
    assert_eq!(calls.visibility.get(), 0);
    let pointer = std::ptr::NonNull::<u8>::dangling().as_ptr().cast();
    let parent = if platform_api() == CLAP_WINDOW_API_COCOA {
        WindowHandle::NSView(pointer)
    } else if platform_api() == CLAP_WINDOW_API_WIN32 {
        WindowHandle::HWND(pointer)
    } else {
        WindowHandle::X11(1)
    };
    instance.open(parent, 2.0).unwrap();
    let host = instance.host_context.host_ptr();
    let timer_host =
        unsafe { ((*host).get_extension.unwrap())(host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) }
            .cast::<clap_host_timer_support>();
    let register_timer = unsafe { (*timer_host).register_timer.unwrap() };
    let unregister_timer = unsafe { (*timer_host).unregister_timer.unwrap() };
    let mut timer_id = clap_sys::id::CLAP_INVALID_ID;
    assert!(unsafe { register_timer(host, 33, &raw mut timer_id) });
    assert_eq!(
        calls.scale.get(),
        usize::from(platform_api() != CLAP_WINDOW_API_COCOA)
    );
    instance.hide();
    instance.show();
    instance.close();
    assert!(unsafe { unregister_timer(host, timer_id) });
    let closed_count = calls.visibility.get();
    assert!(closed_count >= 3);
    instance.hide();
    instance.show();
    assert_eq!(calls.visibility.get(), closed_count);
}

#[test]
fn editor_resize_honors_capability_and_adjustment_rejection() {
    use truce_rack_core::PluginEditor;

    struct Resize {
        resizable: bool,
        accepted: bool,
        adjusted: (u32, u32),
        calls: Cell<usize>,
        applied: Cell<Option<(u32, u32)>>,
    }
    unsafe extern "C" fn can_resize(plugin: *const clap_plugin) -> bool {
        // SAFETY: the fixture retains Resize in plugin_data through every GUI call.
        unsafe { &*(*plugin).plugin_data.cast::<Resize>() }.resizable
    }
    unsafe extern "C" fn adjust(
        plugin: *const clap_plugin,
        width: *mut u32,
        height: *mut u32,
    ) -> bool {
        // SAFETY: the fixture retains Resize and the adapter supplies live size outputs.
        unsafe {
            let state = &*(*plugin).plugin_data.cast::<Resize>();
            *width = state.adjusted.0;
            *height = state.adjusted.1;
            state.accepted
        }
    }
    unsafe extern "C" fn set(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
        // SAFETY: the fixture retains Resize; GUI calls are serialized on this thread.
        let state = unsafe { &*(*plugin).plugin_data.cast::<Resize>() };
        state.calls.set(state.calls.get() + 1);
        state.applied.set(Some((width, height)));
        true
    }
    for (resizable, accepted, adjusted, expected) in [
        (false, true, (800, 600), None),
        (true, false, (800, 600), None),
        (true, true, (0, 600), None),
        (true, true, (800, 0), None),
        (true, true, (800, 600), Some((800, 600))),
    ] {
        let state = Resize {
            resizable,
            accepted,
            adjusted,
            calls: Cell::new(0),
            applied: Cell::new(None),
        };
        // SAFETY: CLAP callbacks are nullable; this fixture installs the callbacks it uses.
        let mut native: clap_plugin = unsafe { std::mem::zeroed() };
        native.plugin_data = std::ptr::from_ref(&state).cast_mut().cast();
        // SAFETY: absent GUI callbacks have a valid null representation.
        let mut gui: clap_plugin_gui = unsafe { std::mem::zeroed() };
        gui.can_resize = Some(can_resize);
        gui.adjust_size = Some(adjust);
        gui.set_size = Some(set);
        let mut instance = active_process_test_instance(&native, process_layout(2, 2));
        instance.editor.extension = &raw const gui;
        assert_eq!(instance.set_size(900, 700), None);
        instance.editor.open = true;
        for (width, height) in [(0, 700), (900, 0)] {
            assert_eq!(instance.set_size(width, height), None);
        }
        assert_eq!(state.calls.get(), 0);
        assert_eq!(instance.set_size(900, 700), expected);
        assert_eq!(state.calls.get(), usize::from(expected.is_some()));
        assert_eq!(state.applied.get(), expected);
        instance.editor.open = false;
    }
}

mod gui_request_fixture {
    use super::*;

    pub(super) struct GuiCalls {
        pub host: Cell<*const clap_host>,
        pub size: Cell<(u32, u32)>,
        pub resize_accepted: Cell<bool>,
        pub destroy_count: Cell<usize>,
        pub hide_count: Cell<usize>,
    }

    unsafe fn calls<'a>(plugin: *const clap_plugin) -> &'a GuiCalls {
        // SAFETY: the test keeps GuiCalls alive until after the instance is destroyed.
        unsafe { &*(*plugin).plugin_data.cast::<GuiCalls>() }
    }
    unsafe extern "C" fn create(_: *const clap_plugin, _: *const c_char, _: bool) -> bool {
        true
    }
    unsafe extern "C" fn parent(_: *const clap_plugin, _: *const clap_window) -> bool {
        true
    }
    unsafe extern "C" fn show(plugin: *const clap_plugin) -> bool {
        // SAFETY: the live fixture owns its host context through this synchronous callback.
        let calls = unsafe { calls(plugin) };
        let host = calls.host.get();
        let gui = unsafe { (*host).get_extension.unwrap()(host, CLAP_EXT_GUI.as_ptr()) }
            .cast::<clap_sys::ext::gui::clap_host_gui>();
        let accepted = unsafe { (*gui).request_resize.unwrap()(host, 1000, 600) };
        calls.resize_accepted.set(accepted);
        true
    }
    unsafe extern "C" fn get_size(
        plugin: *const clap_plugin,
        width: *mut u32,
        height: *mut u32,
    ) -> bool {
        // SAFETY: the adapter supplies live output pointers and retains the fixture.
        let size = unsafe { calls(plugin) }.size.get();
        unsafe {
            *width = size.0;
            *height = size.1;
        }
        true
    }
    unsafe extern "C" fn set_size(plugin: *const clap_plugin, width: u32, height: u32) -> bool {
        // SAFETY: the fixture remains alive for this serialized GUI call.
        unsafe { calls(plugin) }.size.set((width, height));
        true
    }
    unsafe extern "C" fn can_resize(_: *const clap_plugin) -> bool {
        true
    }
    unsafe extern "C" fn hide(plugin: *const clap_plugin) -> bool {
        // SAFETY: the fixture remains alive for this serialized GUI call.
        let calls = unsafe { calls(plugin) };
        calls.hide_count.set(calls.hide_count.get() + 1);
        true
    }
    unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
        // SAFETY: the fixture remains alive for this serialized GUI call.
        let calls = unsafe { calls(plugin) };
        calls.destroy_count.set(calls.destroy_count.get() + 1);
    }

    pub(super) fn gui() -> clap_plugin_gui {
        // SAFETY: absent optional GUI callbacks have a valid null representation.
        let mut gui: clap_plugin_gui = unsafe { std::mem::zeroed() };
        gui.create = Some(create);
        gui.set_parent = Some(parent);
        gui.show = Some(show);
        gui.get_size = Some(get_size);
        gui.can_resize = Some(can_resize);
        gui.set_size = Some(set_size);
        gui.hide = Some(hide);
        gui.destroy = Some(destroy);
        gui
    }
}

#[test]
fn plugin_resize_request_reaches_editor_size_without_reentering_plugin_gui() {
    use gui_request_fixture::GuiCalls;
    use truce_rack_core::{PluginEditor, WindowHandle};

    let calls = GuiCalls {
        host: Cell::new(ptr::null()),
        size: Cell::new((640, 480)),
        resize_accepted: Cell::new(false),
        destroy_count: Cell::new(0),
        hide_count: Cell::new(0),
    };
    // SAFETY: nullable callbacks and pointers in this fixture may be zero-initialized.
    let mut native: clap_plugin = unsafe { std::mem::zeroed() };
    native.plugin_data = ptr::from_ref(&calls).cast_mut().cast();
    let gui = gui_request_fixture::gui();
    let mut instance = active_process_test_instance(&native, process_layout(2, 2));
    instance.editor.extension = &raw const gui;
    calls.host.set(instance.host_context.host_ptr());
    let pointer = std::ptr::NonNull::<u8>::dangling().as_ptr().cast();
    let parent = if platform_api() == CLAP_WINDOW_API_COCOA {
        WindowHandle::NSView(pointer)
    } else if platform_api() == CLAP_WINDOW_API_WIN32 {
        WindowHandle::HWND(pointer)
    } else {
        WindowHandle::X11(1)
    };

    instance.open(parent, 1.0).unwrap();
    assert!(calls.resize_accepted.get());
    assert_eq!(instance.size(), Some((1000, 600)));
    assert_eq!(calls.size.get(), (640, 480));
    assert_eq!(instance.set_size(700, 500), Some((700, 500)));
    assert_eq!(instance.size(), Some((700, 500)));

    let host = calls.host.get();
    let host_gui = unsafe { (*host).get_extension.unwrap()(host, CLAP_EXT_GUI.as_ptr()) }
        .cast::<clap_sys::ext::gui::clap_host_gui>();
    unsafe { (*host_gui).closed.unwrap()(host, true) };
    instance.on_idle();
    assert!(!instance.is_open());
    assert_eq!(instance.size(), None);
    assert_eq!(calls.hide_count.get(), 0);
    assert_eq!(calls.destroy_count.get(), 1);
    assert!(!unsafe { (*host_gui).request_resize.unwrap()(host, 800, 500) });
}
