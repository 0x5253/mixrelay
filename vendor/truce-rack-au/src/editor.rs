//! Cocoa and `AUv3` editor views owned by the main thread.

use crate::AuPlugin;
use crate::au3_editor;
use objc2_audio_toolbox::{
    AudioComponentInstance, AudioUnitGetProperty, AudioUnitGetPropertyInfo, kAudioUnitScope_Global,
};
use std::ptr;
use truce_rack_core::editor::WindowHandle;
use truce_rack_core::error::{Error, Result};

/// Main-thread ownership of one AU's Cocoa factory, controller and attached view.
#[derive(Default)]
pub(super) struct AuEditorState {
    pub(super) factory: Option<objc2::rc::Retained<objc2::runtime::AnyObject>>,
    pub(super) controller: Option<objc2::rc::Retained<objc2_app_kit::NSViewController>>,
    pub(super) view: Option<objc2::rc::Retained<objc2_app_kit::NSView>>,
}

/// Whether the AU advertises a Cocoa view factory implementing `AUCocoaUIBase`.
pub(super) unsafe fn has_cocoa_ui(unit: AudioComponentInstance) -> bool {
    use objc2_audio_toolbox::kAudioUnitProperty_CocoaUI;
    let mut size: u32 = 0;
    let info_status = unsafe {
        AudioUnitGetPropertyInfo(
            unit,
            kAudioUnitProperty_CocoaUI,
            kAudioUnitScope_Global,
            0,
            &raw mut size,
            ptr::null_mut(),
        )
    };
    info_status == 0 && size as usize >= std::mem::size_of::<usize>() * 2
}
impl truce_rack_core::editor::PluginEditor for AuPlugin {
    fn open(&mut self, parent: truce_rack_core::editor::WindowHandle, _scale: f64) -> Result<()> {
        self.ensure_creator_thread()?;
        let _main = objc2::MainThreadMarker::new()
            .ok_or_else(|| Error::Other("AU editor requires the macOS main thread".into()))?;
        let WindowHandle::NSView(parent_ptr) = parent else {
            return Err(Error::Other("AU editor requires an NSView parent".into()));
        };
        if parent_ptr.is_null() {
            return Err(Error::Other("AU editor: parent NSView is null".into()));
        }
        if self.editor.view.is_some() {
            return Ok(());
        }
        // Creating the native view may expose plug-in state that changes latency, even if
        // creation later fails partway through.
        self.invalidate_latency();
        if self.is_v3 {
            unsafe { open_au3_editor(self, parent_ptr) }
        } else {
            unsafe { open_cocoa_editor(self, parent_ptr) }
        }
    }

    fn close(&mut self) {
        if self.ensure_creator_thread().is_err() || objc2::MainThreadMarker::new().is_none() {
            return;
        }
        if let Some(view) = self.editor.view.take() {
            unsafe { remove_from_superview(&view) };
        }
        self.editor.factory = None;
        self.editor.controller = None;
        self.invalidate_latency();
    }

    fn is_open(&self) -> bool {
        self.ensure_creator_thread().is_ok() && self.editor.view.is_some()
    }

    fn size(&self) -> Option<(u32, u32)> {
        self.ensure_creator_thread().ok()?;
        let view = self.editor.view.as_ref()?;
        let frame = unsafe { view_frame(view) };
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        Some((frame.2.max(0.0) as u32, frame.3.max(0.0) as u32))
    }

    fn is_resizable(&self) -> bool {
        if self.ensure_creator_thread().is_err() {
            return false;
        }
        // AUv2 Cocoa views are typically fixed-size: the factory
        // returns an NSView with a baked layout. Resizing is
        // accepted by setFrame: but most hosts treat AUv2 views as
        // non-resizable. We mirror that.
        false
    }

    fn set_size(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        self.ensure_creator_thread().ok()?;
        let view = self.editor.view.as_ref()?;
        unsafe { set_view_frame(view, f64::from(width), f64::from(height)) };
        Some((width, height))
    }

    fn show(&mut self) {
        if self.ensure_creator_thread().is_err() {
            return;
        }
        if let Some(view) = self.editor.view.as_ref() {
            unsafe { set_hidden(view, false) };
        }
    }

    fn hide(&mut self) {
        if self.ensure_creator_thread().is_err() {
            return;
        }
        if let Some(view) = self.editor.view.as_ref() {
            unsafe { set_hidden(view, true) };
        }
    }
}

/// Signature of `[AUCocoaUIBase uiViewForAudioUnit:withSize:]`.
/// Used to dispatch directly through the method's raw IMP, side-
/// stepping objc2's encoding check (Apple's AUs use one type
/// encoding for the `AudioUnit` argument, third-party AUs use the
/// older Carbon-era one — same ABI, but objc2 panics on mismatch).
pub(super) type UiViewFn = unsafe extern "C-unwind" fn(
    *mut objc2::runtime::AnyObject,
    objc2::runtime::Sel,
    *mut std::ffi::c_void,
    objc2_foundation::NSSize,
) -> *mut objc2_app_kit::NSView;

/// Ask an `AUv3` for its native view controller and attach its view to the existing
/// host-owned `NSView`. The controller remains retained for as long as the editor is open.
pub(super) unsafe fn open_au3_editor(
    plugin: &mut AuPlugin,
    parent_ptr: *mut std::ffi::c_void,
) -> Result<()> {
    // SAFETY: the plug-in owns a live instance, and open() runs on its creator/main thread.
    let controller = unsafe { au3_editor::request_view_controller(plugin.instance)? };
    let view = controller.view();
    // SAFETY: open() validated the non-null host NSView, and all AppKit calls are on
    // the creator/main thread. addSubview: retains the view independently.
    let parent = parent_ptr.cast::<objc2_app_kit::NSView>();
    let _: () = unsafe { objc2::msg_send![parent, addSubview: &*view] };
    plugin.editor.controller = Some(controller);
    plugin.editor.view = Some(view);
    Ok(())
}

/// Fetch the AU's Cocoa view factory descriptor, load the bundle,
/// instantiate the factory, ask it for an `NSView`, and add it as a
/// subview of `parent_ptr`. Stores the live factory + view on
/// `self.editor` so they survive `open` returning.
pub(super) unsafe fn open_cocoa_editor(
    plugin: &mut AuPlugin,
    parent_ptr: *mut std::ffi::c_void,
) -> Result<()> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_core_foundation::CFRetained;

    let (bundle_url, class_name) = unsafe { cocoa_descriptor(plugin.instance)? };

    // CFURL ↔ NSURL and CFString ↔ NSString are toll-free
    // bridged, but objc2's msg_send! checks the type encoding —
    // raw CFType pointers register as struct pointers, not '@'.
    // Cast through the matching NSObject type so the encoding
    // matches `id` (and any future runtime type checks pass).
    // SAFETY: toll-free bridging guarantees identical layout.
    let bundle_url_ns: &objc2_foundation::NSURL = unsafe {
        &*CFRetained::as_ptr(&bundle_url)
            .as_ptr()
            .cast::<objc2_foundation::NSURL>()
    };
    let class_name_ns: &objc2_foundation::NSString = unsafe {
        &*CFRetained::as_ptr(&class_name)
            .as_ptr()
            .cast::<objc2_foundation::NSString>()
    };

    // [NSBundle bundleWithURL:url] → NSBundle*
    let ns_bundle_class = objc2::class!(NSBundle);
    let bundle: *mut AnyObject =
        unsafe { msg_send![ns_bundle_class, bundleWithURL: bundle_url_ns] };
    if bundle.is_null() {
        return Err(Error::Other("NSBundle bundleWithURL: returned nil".into()));
    }
    // [bundle classNamed:name] → Class
    let view_factory_class: *mut objc2::runtime::AnyClass =
        unsafe { msg_send![bundle, classNamed: class_name_ns] };
    if view_factory_class.is_null() {
        return Err(Error::Other(format!(
            "view factory class not found in bundle: {}",
            unsafe { (*CFRetained::as_ptr(&class_name).as_ptr()).to_string() }
        )));
    }
    // [[class alloc] init]
    let factory_alloc: *mut AnyObject = unsafe { msg_send![view_factory_class, alloc] };
    let factory: *mut AnyObject = unsafe { msg_send![factory_alloc, init] };
    if factory.is_null() {
        return Err(Error::Other("AU view factory init returned nil".into()));
    }
    let factory_retained = unsafe { objc2::rc::Retained::from_raw(factory) }
        .ok_or_else(|| Error::Other("could not retain AU view factory".into()))?;

    // Default preferred size — most factories ignore this and pick
    // their own.
    let pref = objc2_foundation::NSSize::new(0.0, 0.0);
    // `uiViewForAudioUnit:withSize:` was historically declared with
    // the Carbon-era `^{ComponentInstanceRecord=[1q]}` encoding;
    // Apple's own AU factories migrated to the newer
    // `^{OpaqueAudioComponentInstance=}` form. objc2's msg_send!
    // panics on encoding mismatch. Skip the check by dispatching
    // through the method's raw IMP directly — same ABI, no encoding
    // verification.
    let sel = objc2::sel!(uiViewForAudioUnit:withSize:);
    let factory_class = (*factory_retained).class();
    let Some(method) = factory_class.instance_method(sel) else {
        return Err(Error::Other(
            "AU view factory has no uiViewForAudioUnit:withSize: method".into(),
        ));
    };
    let imp = method.implementation();
    // SAFETY: AUv2 view factories all expose this exact signature;
    // we matched the selector above before transmuting the IMP.
    let typed: UiViewFn = unsafe { std::mem::transmute(imp) };
    let factory_obj: *mut objc2::runtime::AnyObject =
        objc2::rc::Retained::as_ptr(&factory_retained).cast_mut();
    let view_raw: *mut objc2_app_kit::NSView =
        unsafe { typed(factory_obj, sel, plugin.instance.cast(), pref) };
    if view_raw.is_null() {
        return Err(Error::Other(
            "uiViewForAudioUnit:withSize: returned nil".into(),
        ));
    }
    // The returned view is autoreleased; retain it.
    let view = unsafe { objc2::rc::Retained::retain(view_raw) }
        .ok_or_else(|| Error::Other("could not retain AU NSView".into()))?;

    // [parent addSubview:view]
    let parent_view: *mut objc2_app_kit::NSView = parent_ptr.cast();
    let _: () = unsafe { msg_send![parent_view, addSubview: &*view] };

    plugin.editor.factory = Some(factory_retained);
    plugin.editor.view = Some(view);
    Ok(())
}

/// Own the Cocoa descriptor's retained bundle URL and first view-factory class name.
///
/// # Safety
/// `unit` is live and exclusively accessed on the creator/main thread.
unsafe fn cocoa_descriptor(
    unit: AudioComponentInstance,
) -> Result<(
    objc2_core_foundation::CFRetained<objc2_core_foundation::CFURL>,
    objc2_core_foundation::CFRetained<objc2_core_foundation::CFString>,
)> {
    use objc2_audio_toolbox::{AudioUnitCocoaViewInfo, kAudioUnitProperty_CocoaUI};
    use objc2_core_foundation::{CFRetained, CFString, CFURL};
    let mut size: u32 = 0;
    let info_status = unsafe {
        AudioUnitGetPropertyInfo(
            unit,
            kAudioUnitProperty_CocoaUI,
            kAudioUnitScope_Global,
            0,
            &raw mut size,
            ptr::null_mut(),
        )
    };
    if info_status != 0 || (size as usize) < std::mem::size_of::<AudioUnitCocoaViewInfo>() {
        return Err(Error::Other(format!(
            "kAudioUnitProperty_CocoaUI not advertised (status {info_status}, size {size})"
        )));
    }
    // The CocoaUI struct is variable-length: 1 bundle URL +
    // N class names. We get the static-sized struct here and only
    // use the first class name (mCocoaAUViewClass[0]) — the
    // overwhelming majority of AUs publish exactly one view class.
    let mut storage = vec![0u8; size as usize];
    let mut io_size = size;
    let get_status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_CocoaUI,
            kAudioUnitScope_Global,
            0,
            ptr::NonNull::new_unchecked(storage.as_mut_ptr().cast()),
            ptr::NonNull::new_unchecked(&raw mut io_size),
        )
    };
    if get_status != 0 {
        return Err(Error::Other(format!(
            "kAudioUnitProperty_CocoaUI get failed: OSStatus {get_status}"
        )));
    }
    // SAFETY: The first sizeof(AudioUnitCocoaViewInfo) bytes form
    // the descriptor; we've verified size at the property-info
    // step. The pointers inside are +1 references owned by us —
    // we wrap them in CFRetained to release on drop.
    #[allow(clippy::cast_ptr_alignment)]
    let view_info = unsafe { &*storage.as_ptr().cast::<AudioUnitCocoaViewInfo>() };
    let bundle_url = unsafe { CFRetained::<CFURL>::from_raw(view_info.mCocoaAUViewBundleLocation) };
    let class_name = unsafe { CFRetained::<CFString>::from_raw(view_info.mCocoaAUViewClass[0]) };

    Ok((bundle_url, class_name))
}

pub(super) unsafe fn remove_from_superview(view: &objc2_app_kit::NSView) {
    use objc2::msg_send;
    let _: () = unsafe { msg_send![view, removeFromSuperview] };
}

pub(super) unsafe fn set_hidden(view: &objc2_app_kit::NSView, hidden: bool) {
    use objc2::msg_send;
    let flag: objc2::runtime::Bool = objc2::runtime::Bool::new(hidden);
    let _: () = unsafe { msg_send![view, setHidden: flag] };
}

pub(super) unsafe fn view_frame(view: &objc2_app_kit::NSView) -> (f64, f64, f64, f64) {
    use objc2::msg_send;
    use objc2_foundation::NSRect;

    // Force any pending Auto Layout pass so the descendant frames
    // we're about to walk are post-layout. Apple's stock AUv2 views
    // (AULowpass, AUMultibandCompressor, …) only finalize their
    // child-view positions on the first layout pass.
    let _: () = unsafe { msg_send![view, layoutSubtreeIfNeeded] };

    let r: NSRect = unsafe { msg_send![view, frame] };

    // AUGenericView and friends report a `frame` that's smaller
    // than the union of their descendant subviews — macOS doesn't
    // clip children to a parent's bounds by default, so the visible
    // UI extends past the parent's frame. Walk the subtree and use
    // the bounding box of every descendant frame as the real
    // preferred size.
    let (extent_w, extent_h) = unsafe { subtree_extent(view) };
    let w = extent_w.max(r.size.width);
    let h = extent_h.max(r.size.height);
    (r.origin.x, r.origin.y, w, h)
}

/// Recursively walk `view`'s subviews, computing the bounding box
/// (in `view`'s coordinate system) of every descendant's frame.
/// Returns `(max_x, max_y)` — i.e. the max width/height the view
/// would need so no descendant overflows.
pub(super) unsafe fn subtree_extent(view: &objc2_app_kit::NSView) -> (f64, f64) {
    use objc2::msg_send;
    use objc2_foundation::{NSArray, NSRect};

    let subviews: *mut NSArray<objc2_app_kit::NSView> = unsafe { msg_send![view, subviews] };
    if subviews.is_null() {
        return (0.0, 0.0);
    }
    let count: usize = unsafe { msg_send![subviews, count] };
    let mut max_x = 0.0_f64;
    let mut max_y = 0.0_f64;
    for i in 0..count {
        let sub: *mut objc2_app_kit::NSView = unsafe { msg_send![subviews, objectAtIndex: i] };
        if sub.is_null() {
            continue;
        }
        let frame: NSRect = unsafe { msg_send![sub, frame] };
        max_x = max_x.max(frame.origin.x + frame.size.width);
        max_y = max_y.max(frame.origin.y + frame.size.height);
        // Recurse into the descendant, translating its extent into
        // `view`'s coordinate space.
        let (sub_w, sub_h) = unsafe { subtree_extent(&*sub) };
        max_x = max_x.max(frame.origin.x + sub_w);
        max_y = max_y.max(frame.origin.y + sub_h);
    }
    (max_x, max_y)
}

pub(super) unsafe fn set_view_frame(view: &objc2_app_kit::NSView, w: f64, h: f64) {
    use objc2::msg_send;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let r = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h));
    let _: () = unsafe { msg_send![view, setFrame: r] };
}
