//! Embedded CLAP editor state and native window operations.

use crate::ClapPlugin;
use crate::host::HostContext;
use clap_sys::ext::gui::{
    CLAP_WINDOW_API_COCOA, CLAP_WINDOW_API_WIN32, CLAP_WINDOW_API_X11, clap_plugin_gui,
    clap_window, clap_window_handle,
};
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;
use truce_rack_core::error::{Error, Result};

pub(super) struct EditorState {
    pub(super) extension: *const clap_plugin_gui,
    pub(super) open: bool,
}
/// Platform API id we'd ask the plugin to support — one per
/// build target.
pub(super) const fn platform_api() -> &'static CStr {
    #[cfg(target_os = "macos")]
    {
        CLAP_WINDOW_API_COCOA
    }
    #[cfg(target_os = "windows")]
    {
        CLAP_WINDOW_API_WIN32
    }
    #[cfg(target_os = "linux")]
    {
        CLAP_WINDOW_API_X11
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        CLAP_WINDOW_API_COCOA
    }
}

pub(super) unsafe fn gui_supports_current_platform(
    plugin: *const clap_plugin,
    gui_ext: *const clap_plugin_gui,
) -> bool {
    let Some(is_supported) = (unsafe { (*gui_ext).is_api_supported }) else {
        return false;
    };
    unsafe { is_supported(plugin, platform_api().as_ptr(), false) }
}

pub(super) fn handle_to_clap_window(
    handle: truce_rack_core::editor::WindowHandle,
) -> Result<clap_window> {
    use truce_rack_core::editor::WindowHandle;
    let (api, specific) = match handle {
        WindowHandle::NSView(p) => (CLAP_WINDOW_API_COCOA, clap_window_handle { cocoa: p }),
        WindowHandle::HWND(p) => (CLAP_WINDOW_API_WIN32, clap_window_handle { win32: p }),
        WindowHandle::X11(id) => {
            // CLAP uses C unsigned long: 32 bits on Windows and 32-bit Unix,
            // but 64 bits on the other supported Unix targets.
            #[cfg(any(target_os = "windows", target_pointer_width = "32"))]
            let id = u32::try_from(id)
                .map_err(|_| Error::Other("X11 window handle exceeds the CLAP ABI width".into()))?;
            (CLAP_WINDOW_API_X11, clap_window_handle { x11: id })
        }
    };
    // `create` and `set_parent` must agree on the window-system API.
    if api != platform_api() {
        return Err(Error::Other(
            "Parent window API differs from the CLAP editor API".into(),
        ));
    }
    Ok(clap_window {
        api: api.as_ptr(),
        specific,
    })
}

impl EditorState {
    fn open(
        &mut self,
        target: EditorTarget<'_>,
        parent: truce_rack_core::editor::WindowHandle,
        scale: f64,
    ) -> truce_rack_core::error::Result<()> {
        let _main_thread = target.host.main_thread_scope()?;
        if self.open {
            return Ok(());
        }
        if self.extension.is_null() {
            return Err(Error::Other("clap.gui extension absent".into()));
        }
        target.host.reset_gui();
        let window = handle_to_clap_window(parent)?;
        let api = platform_api();
        let create = unsafe { (*self.extension).create }
            .ok_or_else(|| Error::Other("clap.gui missing `create`".into()))?;
        if !unsafe { create(target.plugin, api.as_ptr(), false) } {
            return Err(Error::Other("clap.gui::create returned false".into()));
        }
        self.open = true;
        // CLAP Cocoa sizes use logical pixels; AppKit owns the backing scale. Calling
        // set_scale here can also trigger a vendor resize callback before embedding.
        if api != CLAP_WINDOW_API_COCOA
            && let Some(set_scale) = unsafe { (*self.extension).set_scale }
        {
            let _ = unsafe { set_scale(target.plugin, scale) };
        }
        let Some(set_parent) = (unsafe { (*self.extension).set_parent }) else {
            let _ = close_gui_native(target.plugin, target.host, self.extension, &mut self.open);
            return Err(Error::Other("clap.gui missing `set_parent`".into()));
        };
        if !unsafe { set_parent(target.plugin, &raw const window) } {
            let _ = close_gui_native(target.plugin, target.host, self.extension, &mut self.open);
            return Err(Error::Other("clap.gui::set_parent returned false".into()));
        }
        target.host.gui_parent_attached();
        if let Some(show) = unsafe { (*self.extension).show }
            && !unsafe { show(target.plugin) }
        {
            let _ = close_gui_native(target.plugin, target.host, self.extension, &mut self.open);
            return Err(Error::Other("clap.gui::show returned false".into()));
        }
        Ok(())
    }

    fn close(&mut self, target: EditorTarget<'_>) {
        let _ = close_gui_native(target.plugin, target.host, self.extension, &mut self.open);
        // Keep the cached value unless the plugin's latency/restart callbacks say it changed.
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn size(&self, target: EditorTarget<'_>) -> Option<(u32, u32)> {
        let _main_thread = target.host.main_thread_scope().ok()?;
        if !self.open {
            return None;
        }
        if let Some(size) = target.host.requested_gui_size() {
            return Some(size);
        }
        let get_size = unsafe { (*self.extension).get_size }?;
        let mut w: u32 = 0;
        let mut h: u32 = 0;
        if unsafe { get_size(target.plugin, &raw mut w, &raw mut h) } {
            Some((w, h))
        } else {
            None
        }
    }

    fn is_resizable(&self, target: EditorTarget<'_>) -> bool {
        let Ok(_main_thread) = target.host.main_thread_scope() else {
            return false;
        };
        if !self.open {
            return false;
        }
        unsafe { (*self.extension).can_resize }.is_some_and(|f| unsafe { f(target.plugin) })
    }

    fn set_size(
        &mut self,
        target: EditorTarget<'_>,
        width: u32,
        height: u32,
    ) -> Option<(u32, u32)> {
        let _main_thread = target.host.main_thread_scope().ok()?;
        if !self.open || width == 0 || height == 0 || !self.is_resizable(target) {
            return None;
        }
        let mut w = width;
        let mut h = height;
        // SAFETY: the open GUI extension and plugin remain owned by this instance, and the
        // main-thread scope serializes GUI calls. Size outputs live through the callback.
        if let Some(adjust) = unsafe { (*self.extension).adjust_size }
            && !unsafe { adjust(target.plugin, &raw mut w, &raw mut h) }
        {
            return None;
        }
        if w == 0 || h == 0 {
            return None;
        }
        let set = unsafe { (*self.extension).set_size }?;
        // If set_size itself requests another size, that newer request must survive this call.
        target.host.clear_gui_size_request();
        if unsafe { set(target.plugin, w, h) } {
            Some((w, h))
        } else {
            None
        }
    }

    fn show(&mut self, target: EditorTarget<'_>) {
        if !self.open {
            return;
        }
        let Ok(_main_thread) = target.host.main_thread_scope() else {
            return;
        };
        if let Some(show) = unsafe { (*self.extension).show } {
            let _ = unsafe { show(target.plugin) };
        }
    }

    fn hide(&mut self, target: EditorTarget<'_>) {
        if !self.open {
            return;
        }
        let Ok(_main_thread) = target.host.main_thread_scope() else {
            return;
        };
        if let Some(hide) = unsafe { (*self.extension).hide } {
            let _ = unsafe { hide(target.plugin) };
        }
    }

    fn on_idle(&mut self, target: EditorTarget<'_>) {
        if self.open && target.host.gui_closed_requested() {
            // closed() can arrive on any thread. Acknowledge it only on the plugin's main thread;
            // the next editor snapshot then closes the native parent window.
            let _ = close_gui_native(target.plugin, target.host, self.extension, &mut self.open);
        }
    }
}

#[derive(Clone, Copy)]
struct EditorTarget<'a> {
    plugin: *const clap_plugin,
    host: &'a HostContext,
}

impl ClapPlugin {
    fn editor_target(&self) -> EditorTarget<'_> {
        EditorTarget {
            plugin: self.plugin,
            host: &self.host_context,
        }
    }
}

impl truce_rack_core::editor::PluginEditor for ClapPlugin {
    fn open(&mut self, parent: truce_rack_core::editor::WindowHandle, scale: f64) -> Result<()> {
        self.editor.open(
            EditorTarget {
                plugin: self.plugin,
                host: &self.host_context,
            },
            parent,
            scale,
        )
    }

    fn close(&mut self) {
        self.editor.close(EditorTarget {
            plugin: self.plugin,
            host: &self.host_context,
        });
    }

    fn is_open(&self) -> bool {
        self.editor.is_open()
    }
    fn size(&self) -> Option<(u32, u32)> {
        self.editor.size(self.editor_target())
    }
    fn is_resizable(&self) -> bool {
        self.editor.is_resizable(self.editor_target())
    }

    fn set_size(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        self.editor.set_size(
            EditorTarget {
                plugin: self.plugin,
                host: &self.host_context,
            },
            width,
            height,
        )
    }

    fn show(&mut self) {
        self.editor.show(EditorTarget {
            plugin: self.plugin,
            host: &self.host_context,
        });
    }

    fn hide(&mut self) {
        self.editor.hide(EditorTarget {
            plugin: self.plugin,
            host: &self.host_context,
        });
    }

    fn on_idle(&mut self) {
        self.editor.on_idle(EditorTarget {
            plugin: self.plugin,
            host: &self.host_context,
        });
    }
}

pub(super) fn close_gui_native(
    plugin: *const clap_plugin,
    host_context: &HostContext,
    gui_ext: *const clap_plugin_gui,
    gui_open: &mut bool,
) -> Result<()> {
    let _main_thread = host_context.main_thread_scope()?;
    let was_closed = host_context.take_gui_closed();
    host_context.reset_gui();
    if !*gui_open {
        return Ok(());
    }
    if gui_ext.is_null() {
        return Err(Error::Other(
            "open CLAP GUI lost its extension pointer".into(),
        ));
    }
    if !was_closed && let Some(hide) = unsafe { (*gui_ext).hide } {
        let _ = unsafe { hide(plugin) };
    }
    let Some(destroy) = (unsafe { (*gui_ext).destroy }) else {
        return Err(Error::Other(
            "open CLAP GUI missing destroy callback".into(),
        ));
    };
    unsafe { destroy(plugin) };
    *gui_open = false;
    Ok(())
}
