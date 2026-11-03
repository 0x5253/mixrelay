//! Native LV2 editor ownership, callbacks, and host window integration.

use crate::discovery::{node_to_uri_string, path_to_cstring_with_trailing_sep};
use crate::features::{LV2_URID_MAP_URI, UridMap, map_callback};
use crate::ports::PortInfo;
use crate::{Lv2Plugin, pending_controls};
use lv2_raw::{
    core::LV2Feature,
    ui::{LV2UIControllerRaw, LV2UIDescriptorRaw, LV2UIHandle, LV2UIWidget},
    urid::LV2UridMap,
};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
};
use truce_rack_core::{
    editor::{PluginEditor, WindowHandle},
    error::{Error, Result},
};

const LV2_UI_PARENT_URI: &[u8] = b"http://lv2plug.in/ns/extensions/ui#parent\0";
const LV2_UI_RESIZE_URI: &[u8] = b"http://lv2plug.in/ns/extensions/ui#resize\0";
const LV2_UI_IDLE_INTERFACE_URI: &[u8] = b"http://lv2plug.in/ns/extensions/ui#idleInterface\0";
const LV2_UI_RESIZE_INTERFACE_URI: &[u8] = LV2_UI_RESIZE_URI;

#[cfg(target_os = "macos")]
pub(super) const NATIVE_UI_CLASS_URI: &[u8] = b"http://lv2plug.in/ns/extensions/ui#CocoaUI\0";
#[cfg(target_os = "windows")]
pub(super) const NATIVE_UI_CLASS_URI: &[u8] = b"http://lv2plug.in/ns/extensions/ui#WindowsUI\0";
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) const NATIVE_UI_CLASS_URI: &[u8] = b"http://lv2plug.in/ns/extensions/ui#X11UI\0";
/// Metadata about a plugin's native UI bundle, captured at load
/// time. `binary_path` is the absolute filesystem path to the
/// `.so`/`.dylib`/`.dll` that exports `lv2ui_descriptor`.
#[derive(Debug, Clone)]
pub(super) struct UiBundle {
    ui_uri: CString,
    bundle_path: PathBuf,
    binary_path: PathBuf,
}

/// Keep the UI library handle with the selected descriptor until editor cleanup.
fn open_ui_library(bundle: &UiBundle) -> Result<(libloading::Library, *const LV2UIDescriptorRaw)> {
    type DescFn = unsafe extern "C" fn(u32) -> *const LV2UIDescriptorRaw;
    // SAFETY: this handle stays owned by EditorState until the descriptor's cleanup returns.
    let library = unsafe { libloading::Library::new(&bundle.binary_path) }.map_err(|error| {
        Error::Other(format!(
            "lv2 ui dlopen {}: {}",
            bundle.binary_path.display(),
            error
        ))
    })?;
    // SAFETY: the symbol and every returned descriptor/URI are only inspected while their
    // library is open; the matching raw pointer is returned with that same library handle.
    let descriptor = unsafe {
        let symbol: libloading::Symbol<DescFn> = library
            .get(b"lv2ui_descriptor")
            .map_err(|error| Error::Other(format!("lv2 ui_descriptor symbol: {error}")))?;
        let mut chosen = std::ptr::null();
        for index in 0u32..64 {
            let candidate = symbol(index);
            if candidate.is_null() {
                break;
            }
            let uri = (*candidate).uri;
            if !uri.is_null() && CStr::from_ptr(uri) == bundle.ui_uri.as_c_str() {
                chosen = candidate;
                break;
            }
        }
        chosen
    };
    if descriptor.is_null() {
        return Err(Error::Other(format!(
            "no lv2ui_descriptor matching {:?}",
            bundle.ui_uri
        )));
    }
    Ok((library, descriptor))
}

/// Live editor state. Created on `PluginEditor::open`; torn down on
/// `close` or `Drop`. Kept on `Lv2Plugin` so the dropped instance's
/// `lv2ui_descriptor.cleanup` runs before the audio instance is
/// freed.
struct EditorState {
    /// The dlopen'd UI bundle. Held so the descriptor / function
    /// pointers stay valid; dropped after `cleanup` returns.
    _library: libloading::Library,
    descriptor: *const LV2UIDescriptorRaw,
    handle: LV2UIHandle,
    widget: LV2UIWidget,
    /// Self-contained features array passed to `instantiate`.
    /// Boxed so the pointers we recorded into the LV2 plugin stay
    /// valid even if `Lv2Plugin` itself is moved. Held for ownership
    /// only — the UI side reaches into it through the raw feature
    /// pointers it was given at instantiate time.
    _ui_features: Box<UiFeatureStorage>,
    /// Optional `LV2_UI__idleInterface` — non-null if the UI exports
    /// it via `extension_data`. Driven by `on_idle` once per host
    /// frame.
    idle_iface: *const lv2_raw::ui::LV2UIIdleInterface,
    /// Optional `LV2_UI__resize` interface — non-null if the UI
    /// exports it via `extension_data`. Used by `set_size` to push
    /// host-driven resizes to the UI.
    resize_iface: *const Lv2UiResizeInterface,
    /// Snapshot of `control_values` taken at open time and refreshed
    /// every `on_idle` so we can fire `port_event` for any value the
    /// audio thread (or another UI write) has changed since the last
    /// idle tick.
    last_pushed_values: Vec<f32>,
}

/// LV2 host-side `ui:resize` interface — the same shape both the
/// host implements (passed via the feature) and the UI implements
/// (returned from `extension_data`). Non-zero `ui_resize` returns
/// indicate failure.
#[repr(C)]
struct Lv2UiResizeInterface {
    /// Opaque pointer the UI passes back. For host → UI, this is
    /// the UI's own handle.
    handle: *mut c_void,
    ui_resize: extern "C" fn(handle: *mut c_void, width: i32, height: i32) -> i32,
}

/// Heap-stable storage for the LV2 UI feature array. Mirrors the
/// shape of [`Features`] for the audio side.
struct UiFeatureStorage {
    /// Same URID map the audio instance uses — the UI talks to the
    /// host through the same map.
    map_struct: LV2UridMap,
    /// `ui#parent` feature. `data` is a raw pointer to the parent
    /// widget (`NSView`* / HWND / X11 Window).
    parent_feature_data: *mut c_void,
    /// Host-side `ui:resize` callback. The UI calls
    /// `resize_struct.ui_resize(resize_struct.handle, w, h)` to ask
    /// the host to resize. The handle is a pointer to a
    /// `HostResizeNotifier` heap-allocated alongside us; the host
    /// reads the latest requested size out of it via `take_request`.
    resize_struct: Lv2UiResizeInterface,
    resize_notifier: Box<HostResizeNotifier>,
    feature_storage: Vec<LV2Feature>,
    feature_ptrs: Vec<*const LV2Feature>,
}

/// Heap-stable cell the UI's `ui_resize` callback writes its
/// requested dimensions into. The standalone polls this every
/// `on_idle` and applies any pending request.
struct HostResizeNotifier {
    /// `(width, height)` requested by the UI; `None` means no
    /// pending request. Written by the UI thread, read by the host
    /// — both run on the same main thread, but the field is
    /// touched from C code outside Rust's borrow tracking, so we
    /// use `Cell` to make the interior mutability explicit.
    pending: std::cell::Cell<Option<(i32, i32)>>,
}

extern "C" fn host_resize_callback(handle: *mut c_void, width: i32, height: i32) -> i32 {
    if handle.is_null() {
        return 1;
    }
    // SAFETY: `handle` is the boxed `HostResizeNotifier` we stored
    // in the resize_struct at construction. lilv passes it back
    // unchanged.
    let notifier = unsafe { &*(handle as *const HostResizeNotifier) };
    notifier.pending.set(Some((width, height)));
    0
}

/// Heap-stable callback context. The UI may publish while DSP reads control slots;
/// only serialized plugin access transfers mailbox values to those slots.
pub(super) struct Controller {
    pub(super) pending: pending_controls::PendingControls,
}

impl Controller {
    /// Fix the input-port map and mailbox size before any UI callback can be registered.
    pub(super) fn for_ports(ports: &[PortInfo], offsets: &[usize], control_count: usize) -> Self {
        let input_offsets = ports
            .iter()
            .zip(offsets)
            .map(|(port, &offset)| if port.is_input { offset } else { usize::MAX })
            .collect();
        Self {
            pending: pending_controls::PendingControls::new(input_offsets, control_count),
        }
    }
}

extern "C" fn ui_write_callback(
    controller: LV2UIControllerRaw,
    port_index: libc::c_uint,
    buffer_size: libc::c_uint,
    port_protocol: libc::c_uint,
    buffer: *const c_void,
) {
    if controller.is_null() || buffer.is_null() {
        return;
    }
    // Only the implicit float protocol (port_protocol == 0) updates
    // a control port; richer protocols (atom event transfer, etc.)
    // are not yet plumbed back into the audio side.
    if port_protocol != 0 || buffer_size != 4 {
        return;
    }
    // SAFETY: `controller` is the boxed Controller retained until UI cleanup returns;
    // the UI returns that opaque pointer unchanged and its only mutable fields are atomic.
    let ctrl = unsafe { &*(controller as *const Controller) };
    // SAFETY: implicit floatProtocol requires a readable buffer_size == sizeof(float)
    // payload. `read_unaligned` also accepts a UI-provided buffer without f32 alignment.
    let value = unsafe { (buffer as *const f32).read_unaligned() };
    ctrl.pending.publish(port_index as usize, value);
}

impl UiFeatureStorage {
    // The `Box` return is load-bearing: the caller stores raw
    // pointers into this struct's interior fields, so the heap
    // allocation must outlive `Lv2Plugin` moves.
    #[allow(clippy::unnecessary_box_returns)]
    fn new(map_handle: *mut UridMap, parent: *mut c_void) -> Box<Self> {
        let mut boxed = Box::new(UiFeatureStorage {
            map_struct: LV2UridMap {
                handle: std::ptr::null_mut(),
                map: map_callback,
            },
            parent_feature_data: parent,
            resize_struct: Lv2UiResizeInterface {
                handle: std::ptr::null_mut(),
                ui_resize: host_resize_callback,
            },
            resize_notifier: Box::new(HostResizeNotifier {
                pending: std::cell::Cell::new(None),
            }),
            feature_storage: Vec::with_capacity(3),
            feature_ptrs: Vec::with_capacity(4),
        });
        boxed.map_struct.handle = map_handle.cast::<c_void>();
        boxed.resize_struct.handle =
            (&*boxed.resize_notifier) as *const HostResizeNotifier as *mut c_void;
        boxed.feature_storage.push(LV2Feature {
            uri: LV2_URID_MAP_URI.as_ptr().cast::<c_char>(),
            data: (&boxed.map_struct) as *const LV2UridMap as *mut c_void,
        });
        boxed.feature_storage.push(LV2Feature {
            uri: LV2_UI_PARENT_URI.as_ptr().cast::<c_char>(),
            data: boxed.parent_feature_data,
        });
        boxed.feature_storage.push(LV2Feature {
            uri: LV2_UI_RESIZE_URI.as_ptr().cast::<c_char>(),
            data: (&boxed.resize_struct) as *const Lv2UiResizeInterface as *mut c_void,
        });
        for i in 0..boxed.feature_storage.len() {
            boxed.feature_ptrs.push(&boxed.feature_storage[i]);
        }
        boxed.feature_ptrs.push(std::ptr::null());
        boxed
    }
}

/// Walk `plugin`'s UIs for one matching the host's native UI class.
/// Returns the first match's metadata or `None`.
pub(super) unsafe fn discover_ui(
    world: *mut lilv_sys::LilvWorld,
    plugin: *const lilv_sys::LilvPlugin,
) -> Option<UiBundle> {
    let uis = unsafe { lilv_sys::lilv_plugin_get_uis(plugin) };
    if uis.is_null() {
        return None;
    }
    let class_node = unsafe { lilv_sys::lilv_new_uri(world, NATIVE_UI_CLASS_URI.as_ptr().cast()) };
    let mut chosen: Option<UiBundle> = None;
    let mut it = unsafe { lilv_sys::lilv_uis_begin(uis) };
    while !unsafe { lilv_sys::lilv_uis_is_end(uis, it) } {
        let ui = unsafe { lilv_sys::lilv_uis_get(uis, it) };
        if !ui.is_null() && unsafe { lilv_sys::lilv_ui_is_a(ui, class_node) } {
            let uri_str = unsafe {
                let n = lilv_sys::lilv_ui_get_uri(ui);
                node_to_uri_string(n)
            };
            let bundle_path = unsafe { node_uri_to_path(lilv_sys::lilv_ui_get_bundle_uri(ui)) };
            let binary_path = unsafe { node_uri_to_path(lilv_sys::lilv_ui_get_binary_uri(ui)) };
            if let (Ok(uri_c), Some(bundle), Some(binary)) =
                (CString::new(uri_str), bundle_path, binary_path)
            {
                chosen = Some(UiBundle {
                    ui_uri: uri_c,
                    bundle_path: bundle,
                    binary_path: binary,
                });
                break;
            }
        }
        it = unsafe { lilv_sys::lilv_uis_next(uis, it) };
    }
    unsafe {
        lilv_sys::lilv_node_free(class_node);
        lilv_sys::lilv_uis_free(uis);
    }
    chosen
}

unsafe fn node_uri_to_path(node: *const lilv_sys::LilvNode) -> Option<PathBuf> {
    if node.is_null() {
        return None;
    }
    let uri = unsafe { lilv_sys::lilv_node_as_uri(node) };
    if uri.is_null() {
        return None;
    }
    let raw = unsafe { lilv_sys::lilv_file_uri_parse(uri, std::ptr::null_mut()) };
    if raw.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    unsafe { lilv_sys::lilv_free(raw.cast()) };
    Some(PathBuf::from(s))
}

impl PluginEditor for Lv2Plugin {
    fn open(&mut self, parent: WindowHandle, _scale: f64) -> Result<()> {
        let bundle = self
            .ui
            .bundle
            .as_ref()
            .ok_or_else(|| Error::Other("lv2 plugin has no UI".into()))?
            .clone();
        if self.ui.editor.is_some() {
            return Ok(());
        }

        let parent_ptr: *mut c_void = match parent {
            WindowHandle::NSView(p) | WindowHandle::HWND(p) => p,
            // X11 hands us a u64 XID; LV2 X11UI expects it cast
            // to a pointer (lv2 spec: "an X11 Window with the
            // proper visual"). usize cast is identity on 64-bit
            // and zero-extends on 32-bit, both fine.
            WindowHandle::X11(xid) => xid as usize as *mut c_void,
        };

        let (lib, descriptor) = open_ui_library(&bundle)?;

        // Build the UI feature list. URID map shares the same
        // backing UridMap as the audio side so URIs interned by
        // either path stay consistent.
        let map_handle: *mut UridMap = (&mut self.features.urid_map) as *mut UridMap;
        let ui_features = UiFeatureStorage::new(map_handle, parent_ptr);

        // The callback context is heap-stable from load until UI cleanup returns.
        let controller_ptr: *const Controller = self.ui.controller.as_ref();

        let plugin_uri = CString::new(self.info.unique_id.clone())
            .map_err(|_| Error::Other("lv2 plugin uri contains NUL".into()))?;
        let bundle_path_c = path_to_cstring_with_trailing_sep(&bundle.bundle_path)?;

        let mut widget: LV2UIWidget = std::ptr::null_mut();
        let handle = unsafe {
            ((*descriptor).instantiate_raw)(
                descriptor,
                plugin_uri.as_ptr(),
                bundle_path_c.as_ptr(),
                Some(ui_write_callback),
                controller_ptr.cast::<c_void>(),
                &raw mut widget,
                ui_features.feature_ptrs.as_ptr(),
            )
        };
        if handle.is_null() {
            return Err(Error::Other("lv2 ui instantiate returned NULL".into()));
        }

        // For embedded UI types (CocoaUI / WindowsUI / X11UI) the
        // "widget" is the toolkit-native parent-of-the-UI handle
        // (NSView / HWND / X11 Window). The host's parent already
        // contains it as a child after instantiate; we just keep
        // the pointer for size queries.

        // Look up the optional idle / resize extension interfaces
        // the UI may have published. `extension_data` returns a
        // pointer to a shared static struct of function pointers.
        let idle_iface = unsafe {
            if let Some(ext) = (*descriptor).extension_data {
                ext(LV2_UI_IDLE_INTERFACE_URI.as_ptr().cast::<c_char>())
                    as *const lv2_raw::ui::LV2UIIdleInterface
            } else {
                std::ptr::null()
            }
        };
        let resize_iface = unsafe {
            if let Some(ext) = (*descriptor).extension_data {
                ext(LV2_UI_RESIZE_INTERFACE_URI.as_ptr().cast::<c_char>())
                    as *const Lv2UiResizeInterface
            } else {
                std::ptr::null()
            }
        };

        let last_pushed_values = self.bindings.control_values.clone();

        self.ui.editor = Some(EditorState {
            _library: lib,
            descriptor,
            handle,
            widget,
            _ui_features: ui_features,
            idle_iface,
            resize_iface,
            last_pushed_values,
        });
        Ok(())
    }

    fn close(&mut self) {
        self.close_editor();
    }

    fn is_open(&self) -> bool {
        self.ui.editor.is_some()
    }

    fn size(&self) -> Option<(u32, u32)> {
        let state = self.ui.editor.as_ref()?;
        native_widget_size(state.widget)
    }

    fn is_resizable(&self) -> bool {
        // Resizable iff the UI exported a `ui:resize` extension —
        // that's the only way for the host to push a new size.
        self.ui
            .editor
            .as_ref()
            .is_some_and(|e| !e.resize_iface.is_null())
    }

    fn set_size(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let state = self.ui.editor.as_ref()?;
        if state.resize_iface.is_null() {
            return None;
        }
        // SAFETY: `resize_iface` was returned by the UI's
        // `extension_data(ui:resize)` and is a static struct of
        // function pointers owned by the UI bundle (still loaded
        // because `EditorState._library` is alive).
        let r = unsafe {
            ((*state.resize_iface).ui_resize)(
                (*state.resize_iface).handle,
                i32::try_from(width).unwrap_or(i32::MAX),
                i32::try_from(height).unwrap_or(i32::MAX),
            )
        };
        if r == 0 { Some((width, height)) } else { None }
    }

    fn show(&mut self) {
        // Embedded LV2 UIs are visible immediately after
        // instantiate; nothing to do.
    }

    fn hide(&mut self) {
        // No-op for embedded UIs — closing the editor or the host
        // window is the visibility primitive.
    }

    fn on_idle(&mut self) {
        self.drain_pending_controls();
        let Some(state) = self.ui.editor.as_mut() else {
            return;
        };
        // 1. Drive the optional `ui:idleInterface` first — animations
        //    and the UI's own event pump live here. Non-zero return
        //    means the UI closed itself; we bail and let the next
        //    on_idle skip cleanly.
        if !state.idle_iface.is_null() {
            let rc = unsafe { ((*state.idle_iface).idle)(state.handle) };
            if rc != 0 {
                // UI asked to be torn down. close_editor walks the
                // descriptor's cleanup and drops the library.
                self.close_editor();
                return;
            }
        }
        self.drain_pending_controls();

        // 2. Push host-side parameter changes to the UI via
        //    `port_event`. Compare current control_values against
        //    the snapshot we took last tick; for any port whose value
        //    differs, fire a float-protocol port_event so the UI
        //    redraws. Snapshot is then refreshed.
        let Some(state) = self.ui.editor.as_mut() else {
            return;
        };
        // SAFETY: `descriptor` is the same one we instantiated; its
        // `port_event` field is non-null per the LV2 spec.
        let port_event_fn = unsafe { (*state.descriptor).port_event };
        for (port_index, &offset) in self.bindings.control_value_offset.iter().enumerate() {
            if offset == usize::MAX || offset >= self.bindings.control_values.len() {
                continue;
            }
            let cur = self.bindings.control_values[offset];
            // First idle tick after a re-open may have a shorter
            // snapshot than control_values — guard via .get.
            let prev = state
                .last_pushed_values
                .get(offset)
                .copied()
                .unwrap_or(cur + 1.0);
            // f32 inequality is fine here — we only push when the
            // bit pattern actually changed, NaNs included (NaN != NaN
            // is the right answer; both sides will have NaN if the
            // plugin keeps writing NaN, no spurious push).
            #[allow(clippy::float_cmp)]
            let changed = cur != prev;
            if changed {
                let value = cur;
                let port_index_u32 = u32::try_from(port_index).unwrap_or(u32::MAX);
                port_event_fn(
                    state.handle,
                    port_index_u32,
                    4,
                    0, // float protocol
                    (&raw const value).cast::<c_void>(),
                );
                if offset < state.last_pushed_values.len() {
                    state.last_pushed_values[offset] = cur;
                }
            }
        }
        // suppress unused-warning if no port matched
        let _ = port_event_fn;

        // 3. Apply any UI-requested resize. The host-side ui:resize
        //    callback writes into the notifier; on_idle is the host's
        //    chance to act on it. We can't actually resize the
        //    baseview window from inside this trait method (no window
        //    handle), so we just stash the request — windowed.rs polls
        //    `size()` next frame and resizes accordingly.
        // Currently no-op past the notifier write; future host-driven
        // window resize would consume `notifier.pending.take()` here.
    }
}

#[cfg(target_os = "macos")]
fn native_widget_size(widget: LV2UIWidget) -> Option<(u32, u32)> {
    use objc2::msg_send;
    use objc2_foundation::NSRect;
    if widget.is_null() {
        return None;
    }
    // SAFETY: For CocoaUI the widget is an NSView*. Reading
    // -frame is always safe on a live NSView; the LV2 spec
    // promises the widget stays valid until cleanup.
    let view = widget as *mut objc2::runtime::AnyObject;
    let frame: NSRect = unsafe { msg_send![view, frame] };
    // `.max(0.0)` clamps the negative branch the sign-loss lint
    // worries about; window dimensions can't reasonably overflow u32.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    Some((
        frame.size.width.max(0.0) as u32,
        frame.size.height.max(0.0) as u32,
    ))
}

#[cfg(not(target_os = "macos"))]
fn native_widget_size(_widget: LV2UIWidget) -> Option<(u32, u32)> {
    // X11 / Win32 paths would query XGetWindowAttributes /
    // GetClientRect respectively; truce-rack-standalone falls back to
    // its INITIAL_WINDOW until that's wired.
    None
}

#[cfg(test)]
mod tests;

/// One instance's UI metadata, live native editor, and stable control callback context.
pub(super) struct UiOwner {
    pub(super) bundle: Option<UiBundle>,
    editor: Option<EditorState>,
    pub(super) controller: Box<Controller>,
}

impl UiOwner {
    pub(super) fn new(bundle: Option<UiBundle>, controller: Box<Controller>) -> Self {
        Self {
            bundle,
            editor: None,
            controller,
        }
    }

    fn close(&mut self) {
        drop(self.editor.take());
    }
}

impl Drop for EditorState {
    fn drop(&mut self) {
        // SAFETY: this handle was returned by this descriptor, and its library/features
        // are still retained until cleanup returns. UiOwner removes each state only once.
        unsafe { ((*self.descriptor).cleanup)(self.handle) };
    }
}

impl Lv2Plugin {
    pub(super) fn close_editor(&mut self) {
        self.ui.close();
        self.drain_pending_controls();
    }
}
